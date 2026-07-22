//! The read-time price table (ADR-0008 D2/D8). Tokens are the immutable truth in
//! the ledger; USD is a *projection* applied here at read-time and never written.
//! The table is keyed by **model** (opus costs the same per token whoever ran it),
//! ships with sane defaults for the models actually in use, and is
//! operator-overridable at `~/.ralphy/pricing.toml`.
//!
//! A model absent from the table reports **unknown** cost (`None`) and logs "add
//! `<model>` to pricing.toml" — never `0`, which would be a lie that hides spend
//! (the empirical reason: OpenCode's custom model IDs would otherwise report $0
//! for millions of tokens).

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::Deserialize;
use tracing::warn;

use ralphy_core::Usage;

mod defaults;

/// The per-1M-token USD price for one model (ADR-0008 D8). The four fields mirror
/// [`Usage`]'s numeric split so each token kind is priced at its own rate — cache
/// reads in particular are ~1/10th of fresh input, so collapsing them would
/// overstate cost by an order of magnitude.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_creation: f64,
}

/// A model-keyed price table. Built from [`defaults`](PriceTable::defaults) and
/// optionally overlaid with the operator's `~/.ralphy/pricing.toml`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PriceTable(pub BTreeMap<String, ModelPrice>);

/// The set of unknown models already warned about, so the "add `<model>` to
/// pricing.toml" hint is logged once per model rather than on every priced row.
static WARNED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// The bucket key the runner assigns to a usage record with no model attribution
/// (`Usage.model == None`). A sentinel, never a real model id — the pricing layer
/// treats it specially so it is never reported as a model you could add to the table.
const UNKNOWN_MODEL: &str = "unknown";

impl PriceTable {
    /// The read-time USD cost of `tokens` priced as `model`, or `None` when the
    /// model is absent from the table (logged once — never reported as `0`).
    pub fn cost_usd(&self, model: &str, tokens: &Usage) -> Option<f64> {
        let Some(price) = self.resolve(model) else {
            warn_unknown(model);
            return None;
        };
        Some(
            (tokens.input as f64 / 1e6) * price.input
                + (tokens.output as f64 / 1e6) * price.output
                + (tokens.cache_read as f64 / 1e6) * price.cache_read
                + (tokens.cache_creation as f64 / 1e6) * price.cache_creation,
        )
    }

    /// Price a per-model token split (ADR-0008 D8): sum each model's read-time
    /// cost. Returns `(usd, any_unpriced)` where `usd` is `None` when *nothing* in
    /// the set could be priced (rendered `~$?`, never `0`), `Some(sum)` of the
    /// priced portion otherwise; `any_unpriced` flags a model absent from the table
    /// so the figure can carry a `+?` residue marker.
    pub fn cost_usd_by_model(&self, by_model: &BTreeMap<String, Usage>) -> (Option<f64>, bool) {
        let mut usd = 0.0;
        let mut any_priced = false;
        let mut any_unpriced = false;
        for (model, tokens) in by_model {
            // A zero-token entry carries no spend and no signal — skip it so an
            // empty `unknown` bucket never forces a spurious `+?`.
            if tokens.total() == 0 {
                continue;
            }
            match self.cost_usd(model, tokens) {
                Some(c) => {
                    usd += c;
                    any_priced = true;
                }
                None => any_unpriced = true,
            }
        }
        (any_priced.then_some(usd), any_unpriced)
    }

    /// Resolve a model id to its price, tolerating a trailing release-date suffix:
    /// `claude-haiku-4-5-20251001` falls back to the undated family id
    /// `claude-haiku-4-5`. Claude Code keys its `modelUsage` map by the *dated* id
    /// while the table (and Anthropic's published price list) uses the undated
    /// family id, so without this fallback every dated id reports as unpriced
    /// (`~$?`) even when its family is in the table.
    ///
    /// A dotted id falls back to its dashed form too: Copilot's catalog spells the
    /// Anthropic families `claude-haiku-4.5` where the table (and Anthropic) use
    /// `claude-haiku-4-5` — punctuation only. Normalization never invents a price:
    /// an id whose dashed form is also absent still resolves to `None`.
    ///
    /// Finally, a leading `provider/` segment is stripped: the native Kimi run path
    /// emits `kimi-code/k3` while the usage scan (`scan_kimi_code` strips the
    /// prefix) and the table's K2-family convention (`k2p6`, `kimi-k2.7-code`) key
    /// the bare `k3`. Without this the same model prices on a run yet reports
    /// `unknown model` on `ralphy usage` for the identical session (ADR-0028 D4).
    fn resolve(&self, model: &str) -> Option<&ModelPrice> {
        let stripped = strip_release_date(model);
        self.0
            .get(model)
            .or_else(|| self.0.get(stripped))
            .or_else(|| self.0.get(&dots_to_dashes(model)))
            .or_else(|| self.0.get(&dots_to_dashes(stripped)))
            .or_else(|| self.0.get(strip_provider_prefix(model)))
    }

    /// Load the effective table: the shipped [`defaults`](Self::defaults) overlaid
    /// with `~/.ralphy/pricing.toml` when present. The override path is
    /// `$RALPHY_PRICING_FILE` when set (tests point it at a temp file), else
    /// `<home>/.ralphy/pricing.toml`, resolving home via `USERPROFILE`/`HOME` to
    /// match `ledger.rs`. A missing or malformed file leaves the defaults intact.
    pub fn load() -> Self {
        let mut table = Self::defaults();
        let Some(path) = pricing_file() else {
            return table;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return table;
        };
        match toml::from_str::<BTreeMap<String, ModelPrice>>(&text) {
            Ok(overrides) => {
                for (model, price) in overrides {
                    table.0.insert(model, price);
                }
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "parsing pricing.toml failed — using defaults")
            }
        }
        table
    }
}

/// Strip a trailing `-YYYYMMDD` release-date suffix from a model id, returning the
/// undated family id (`claude-haiku-4-5-20251001` → `claude-haiku-4-5`). Returns
/// the input unchanged when the final segment is not exactly eight digits, so a
/// genuinely undated id (or an operator's custom key) is never mangled.
fn strip_release_date(model: &str) -> &str {
    match model.rsplit_once('-') {
        Some((head, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => model,
    }
}

/// A model id with `.` rewritten to `-`, the punctuation-only difference between
/// Copilot's catalog ids and the table's family keys.
fn dots_to_dashes(model: &str) -> String {
    model.replace('.', "-")
}

/// A model id with a leading `provider/` segment removed (`kimi-code/k3` → `k3`),
/// so the native Kimi run path's prefixed id resolves against the same bare
/// K2-family key the usage scan and the table's convention already use. Returns
/// the input unchanged when it carries no `/`, so a slash-free id is never mangled.
fn strip_provider_prefix(model: &str) -> &str {
    model.rsplit_once('/').map_or(model, |(_, tail)| tail)
}

/// Resolve the operator's pricing-override file: `$RALPHY_PRICING_FILE` when set,
/// else `<home>/.ralphy/pricing.toml`. `None` when no home directory resolves.
fn pricing_file() -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("RALPHY_PRICING_FILE") {
        return Some(PathBuf::from(file));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".ralphy").join("pricing.toml"))
}

/// Log a one-shot pricing hint for an unpriced model. The `unknown` *sentinel*
/// (the bucket the runner assigns to a usage record with no model attribution — it
/// is not a real model id) gets a distinct, actionable message instead of the
/// nonsensical "add `unknown` to pricing.toml". Logged at most once per id.
fn warn_unknown(model: &str) {
    let mut seen = WARNED.lock().unwrap_or_else(|e| e.into_inner());
    if seen.insert(model.to_string()) {
        if model == UNKNOWN_MODEL {
            warn!(
                model,
                "some tokens had no model attribution — not priced (shown as +?)"
            );
        } else {
            warn!(
                model,
                "unknown model — add `{model}` to pricing.toml to price it"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialises tests that mutate the process-global `RALPHY_PRICING_FILE`, so
    /// `cargo test`'s parallel runner can't race them (mirrors telegram config).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub(super) fn one_million_each() -> Usage {
        Usage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_creation: 1_000_000,
            model: Some("claude-opus-4-8".into()),
        }
    }

    #[test]
    fn dated_model_id_falls_back_to_undated_family_price() {
        let table = PriceTable::defaults();
        let tokens = one_million_each();
        // The dated id Claude Code keys `modelUsage` by resolves to the undated
        // `claude-haiku-4-5` family price (1 + 5 + 0.1 + 1.25 = 7.35 over 1M each).
        let dated = table
            .cost_usd("claude-haiku-4-5-20251001", &tokens)
            .expect("dated haiku id resolves via family fallback");
        let undated = table
            .cost_usd("claude-haiku-4-5", &tokens)
            .expect("undated haiku is priced");
        assert!(
            (dated - undated).abs() < 1e-9,
            "dated id must price identically to its family, got {dated} vs {undated}"
        );
        // A non-date trailing segment is NOT stripped — a genuinely unknown model
        // stays unknown (never silently mispriced).
        assert_eq!(table.cost_usd("claude-haiku-4-5-turbo", &tokens), None);
        assert_eq!(strip_release_date("claude-opus-4-8"), "claude-opus-4-8");
        assert_eq!(
            strip_release_date("claude-haiku-4-5-20251001"),
            "claude-haiku-4-5"
        );
    }

    #[test]
    fn override_file_reprices_a_model_differently_from_defaults() {
        let _g = ENV_LOCK.lock().unwrap();
        // Write a temp override that doubles opus input, point the loader at it.
        let dir = std::env::temp_dir().join(format!("ralphy-pricing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("pricing.toml");
        std::fs::write(
            &file,
            "[claude-opus-4-8]\ninput = 30.0\noutput = 75.0\ncache_read = 1.5\ncache_creation = 18.75\n",
        )
        .expect("write override");
        std::env::set_var("RALPHY_PRICING_FILE", &file);

        let loaded = PriceTable::load();
        let defaults = PriceTable::defaults();
        let tokens = one_million_each();
        let loaded_cost = loaded.cost_usd("claude-opus-4-8", &tokens).unwrap();
        let default_cost = defaults.cost_usd("claude-opus-4-8", &tokens).unwrap();

        // The override doubled the input rate (15 → 30): +15 over the default.
        assert!(
            (loaded_cost - 125.25).abs() < 1e-9,
            "override reprices to 125.25, got {loaded_cost}"
        );
        assert!(
            (loaded_cost - default_cost).abs() > 1e-9,
            "the override must re-price differently from defaults (read-time projection)"
        );

        std::env::remove_var("RALPHY_PRICING_FILE");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
