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
use std::sync::Mutex;

use serde::Deserialize;
use tracing::warn;

use ralphy_core::Usage;

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
static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

impl PriceTable {
    /// The shipped defaults for the models actually in use. `claude-opus-4-8` is
    /// pinned from ADR-0008 D8 (the canonical test oracle); the sonnet/haiku
    /// entries are indicative current Anthropic list prices and are not asserted
    /// by tests.
    pub fn defaults() -> Self {
        let mut t = BTreeMap::new();
        // ADR-0008 D8 — canonical, the `cost_usd` test oracle.
        t.insert(
            "claude-opus-4-8".to_string(),
            ModelPrice {
                input: 15.0,
                output: 75.0,
                cache_read: 1.5,
                cache_creation: 18.75,
            },
        );
        // Indicative (Anthropic list prices) — not asserted by tests.
        t.insert(
            "claude-sonnet-4-6".to_string(),
            ModelPrice {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_creation: 3.75,
            },
        );
        t.insert(
            "claude-haiku-4-5".to_string(),
            ModelPrice {
                input: 1.0,
                output: 5.0,
                cache_read: 0.1,
                cache_creation: 1.25,
            },
        );
        PriceTable(t)
    }

    /// The read-time USD cost of `tokens` priced as `model`, or `None` when the
    /// model is absent from the table (logged once — never reported as `0`).
    pub fn cost_usd(&self, model: &str, tokens: &Usage) -> Option<f64> {
        let Some(price) = self.0.get(model) else {
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

/// Resolve the operator's pricing-override file: `$RALPHY_PRICING_FILE` when set,
/// else `<home>/.ralphy/pricing.toml`. `None` when no home directory resolves.
fn pricing_file() -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("RALPHY_PRICING_FILE") {
        return Some(PathBuf::from(file));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".ralphy").join("pricing.toml"))
}

/// Log the "add `<model>` to pricing.toml" hint at most once per unknown model.
fn warn_unknown(model: &str) {
    let mut guard = WARNED.lock().unwrap_or_else(|e| e.into_inner());
    let seen = guard.get_or_insert_with(HashSet::new);
    if seen.insert(model.to_string()) {
        warn!(
            model,
            "unknown model — add `{model}` to pricing.toml to price it"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_million_each() -> Usage {
        Usage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_creation: 1_000_000,
            model: Some("claude-opus-4-8".into()),
        }
    }

    #[test]
    fn cost_usd_prices_opus_and_unknown_is_none_never_zero() {
        let table = PriceTable::defaults();
        // 15.0 + 75.0 + 1.5 + 18.75 = 110.25 over 1M of each token kind (D8).
        let opus = table
            .cost_usd("claude-opus-4-8", &one_million_each())
            .expect("opus is priced");
        assert!(
            (opus - 110.25).abs() < 1e-9,
            "opus over 1M-each should be 110.25, got {opus}"
        );
        // An unknown model reports unknown cost — never `Some(0.0)` (ADR-0008 D8).
        assert_eq!(table.cost_usd("big-pickle", &one_million_each()), None);
    }

    #[test]
    fn override_file_reprices_a_model_differently_from_defaults() {
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
