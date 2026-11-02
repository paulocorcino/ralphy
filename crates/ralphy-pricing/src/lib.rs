//! The read-time price table (ADR-0008 D2/D8, ADR-0034 slice A). Tokens are the
//! immutable truth in the ledger; USD is a *projection* applied here at
//! read-time and never written. The floor is an embedded models.dev-shaped seed
//! plus a bare-id slug overlay; an optional disk cache and `pricing.toml`
//! override both. A model absent from every layer reports **unknown** cost
//! (`None`) — never `0`, which would hide spend.
#![cfg_attr(
    test,
    expect(
        clippy::disallowed_methods,
        reason = "nextest runs each test in its own process, so a test may change its environment"
    )
)]

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use tracing::warn;

pub mod fetch;
mod floor;
mod ingest;

/// A usage record's numeric token split, mirroring `ralphy_core::Usage`'s four
/// counts. It exists so this crate carries no `ralphy-core` edge and can be
/// depended on from either side of the ADR-0002 core/adapter seam; the consumer
/// converts at its own boundary. The model id is never part of it — pricing
/// always receives that as a separate `&str`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

impl TokenCounts {
    /// Every token kind summed — a zero total means the record carries no spend.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_creation
    }
}

/// The per-1M-token USD price for one model (ADR-0008 D8). The four fields mirror
/// [`TokenCounts`]'s numeric split so each token kind is priced at its own rate —
/// cache reads in particular are ~1/10th of fresh input, so collapsing them would
/// overstate cost by an order of magnitude.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_creation: f64,
}

/// Operator `pricing.toml`: optional offline gate plus per-model overrides.
#[derive(Debug, Default, Deserialize)]
struct PricingTomlFile {
    #[serde(default)]
    offline: bool,
    #[serde(flatten)]
    overrides: BTreeMap<String, ModelPrice>,
}

/// Disk-cache envelope (ADR-0034 A6): already-normalized `data`, no re-ingest.
#[derive(Debug, Deserialize)]
struct PricingCacheFile {
    #[expect(
        dead_code,
        reason = "read by no code; the envelope keeps the file's own shape"
    )]
    timestamp: String,
    data: BTreeMap<String, ModelPrice>,
}

/// A layered price table. Precedence for a bare id: `pricing.toml` overrides →
/// disk cache (via provider synthesis) → seed (via synthesis) → slug overlay.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PriceTable {
    overrides: BTreeMap<String, ModelPrice>,
    cache: BTreeMap<String, ModelPrice>,
    seed: BTreeMap<String, ModelPrice>,
    overlay: BTreeMap<String, ModelPrice>,
}

/// The set of unknown models already warned about, so the "add `<model>` to
/// pricing.toml" hint is logged once per model rather than on every priced row.
static WARNED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// The bucket key the runner assigns to a usage record with no model attribution
/// (`Usage.model == None`). A sentinel, never a real model id — the pricing layer
/// treats it specially so it is never reported as a model you could add to the table.
const UNKNOWN_MODEL: &str = "unknown";

impl PriceTable {
    /// Build a table from explicit layers — used by [`Self::defaults`] /
    /// [`Self::load`] and by pure resolver unit tests (no disk).
    pub fn from_layers(
        overrides: BTreeMap<String, ModelPrice>,
        cache: BTreeMap<String, ModelPrice>,
        seed: BTreeMap<String, ModelPrice>,
        overlay: BTreeMap<String, ModelPrice>,
    ) -> Self {
        Self {
            overrides,
            cache,
            seed,
            overlay,
        }
    }

    /// The read-time USD cost of `tokens` priced as `model`, or `None` when the
    /// model is absent from every layer (logged once — never reported as `0`).
    pub fn cost_usd(&self, model: &str, tokens: &TokenCounts) -> Option<f64> {
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
    pub fn cost_usd_by_model(
        &self,
        by_model: &BTreeMap<String, TokenCounts>,
    ) -> (Option<f64>, bool) {
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

    /// Resolve a model id through override → cache → seed → overlay, with the
    /// existing release-date / dots-to-dashes / provider-prefix normalizations
    /// applied to bare candidates, and provider-prefix synthesis for catalog keys.
    fn resolve(&self, model: &str) -> Option<&ModelPrice> {
        let candidates = bare_candidates(model);
        first_bare(&self.overrides, &candidates)
            .or_else(|| self.cache_hit(&candidates))
            .or_else(|| self.seed_hit(&candidates))
            .or_else(|| first_bare(&self.overlay, &candidates))
    }

    /// The first candidate whose synthesized catalog key is in the cache, else
    /// whose bare id is.
    fn cache_hit(&self, candidates: &[String]) -> Option<&ModelPrice> {
        candidates.iter().find_map(|c| {
            synthesize(c)
                .and_then(|key| self.cache.get(&key))
                .or_else(|| self.cache.get(c.as_str()))
        })
    }

    /// The first candidate whose synthesized catalog key is in the seed. The
    /// seed is keyed by catalog id only, so a bare id is never looked up.
    fn seed_hit(&self, candidates: &[String]) -> Option<&ModelPrice> {
        candidates
            .iter()
            .find_map(|c| synthesize(c).and_then(|key| self.seed.get(&key)))
    }

    /// Load the effective table: embedded floor, optional disk cache, then
    /// `~/.ralphy/pricing.toml` overrides. Cache path is `$RALPHY_PRICING_CACHE`
    /// when set, else `<home>/.ralphy/pricing-cache/models-dev.json`. Override
    /// path is `$RALPHY_PRICING_FILE` when set, else `<home>/.ralphy/pricing.toml`.
    /// Missing or malformed files leave the lower layers intact — never fetch.
    pub fn load() -> Self {
        let mut table = Self::defaults();
        if let Some(path) = pricing_cache_file() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                match serde_json::from_str::<PricingCacheFile>(&text) {
                    Ok(file) => table.cache = file.data,
                    Err(e) => {
                        warn!(
                            path = %path.display(),
                            error = %e,
                            "parsing pricing cache failed — using seed/overlay floor"
                        );
                    }
                }
            }
        }
        let Some(path) = pricing_file() else {
            return table;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return table;
        };
        match parse_pricing_toml(&text) {
            Ok(file) => {
                table.overrides = file.overrides;
            }
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "parsing pricing.toml failed — using defaults"
                )
            }
        }
        table
    }
}

/// Parse `pricing.toml` text into offline flag + overrides. Prefers a flatten
/// struct; on failure falls back to extracting `offline` from a `toml::Value`
/// then deserializing remaining tables as overrides.
fn parse_pricing_toml(text: &str) -> Result<PricingTomlFile, String> {
    match toml::from_str::<PricingTomlFile>(text) {
        Ok(file) => Ok(file),
        Err(flatten_err) => parse_pricing_table(text, &flatten_err),
    }
}

/// The fallback of [`parse_pricing_toml`]: `offline` read from a `toml::Value`,
/// and every other top-level key deserialized as an override. `flatten_err` is
/// the error of the flatten struct, carried into every message.
fn parse_pricing_table(
    text: &str,
    flatten_err: &toml::de::Error,
) -> Result<PricingTomlFile, String> {
    let value: toml::Value =
        toml::from_str(text).map_err(|e| format!("toml: {e}; flatten: {flatten_err}"))?;
    let offline = value
        .get("offline")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut overrides = BTreeMap::new();
    if let toml::Value::Table(table) = value {
        for (k, v) in table {
            if k == "offline" {
                continue;
            }
            let price = v
                .try_into::<ModelPrice>()
                .map_err(|e| format!("override `{k}`: {e}; flatten: {flatten_err}"))?;
            overrides.insert(k, price);
        }
    }
    Ok(PricingTomlFile { offline, overrides })
}

/// `offline = true` from the resolved `pricing.toml`, or `false` when missing /
/// unreadable / malformed.
pub fn pricing_offline_from_file() -> bool {
    let Some(path) = pricing_file() else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    parse_pricing_toml(&text)
        .map(|f| f.offline)
        .unwrap_or(false)
}

/// The price of the first candidate that is a key of `layer`.
fn first_bare<'a>(
    layer: &'a BTreeMap<String, ModelPrice>,
    candidates: &[String],
) -> Option<&'a ModelPrice> {
    candidates.iter().find_map(|c| layer.get(c.as_str()))
}

/// Bare-id lookup candidates, preserving the historical resolve order.
fn bare_candidates(model: &str) -> Vec<String> {
    let stripped = strip_release_date(model);
    let mut out = Vec::with_capacity(5);
    for c in [
        model.to_string(),
        stripped.to_string(),
        dots_to_dashes(model),
        dots_to_dashes(stripped),
        strip_provider_prefix(model).to_string(),
    ] {
        if !out.iter().any(|e| e == &c) {
            out.push(c);
        }
    }
    out
}

/// Deterministic provider-prefix synthesis (ADR-0034 A2). `None` → overlay path.
pub fn synthesize(id: &str) -> Option<String> {
    let provider = if id.starts_with("claude-") {
        "anthropic"
    } else if id.starts_with("gpt-") {
        "openai"
    } else if id.starts_with("gemini-") {
        "google"
    } else if id.starts_with("kimi-") {
        "moonshotai"
    } else {
        return None;
    };
    Some(format!("{provider}/{id}"))
}

/// Strip a trailing `-YYYYMMDD` release-date suffix from a model id, returning the
/// undated family id (`claude-haiku-4-5-20251001` → `claude-haiku-4-5`). Returns
/// the input unchanged when the final segment is not exactly eight digits, so a
/// genuinely undated id (or an operator's custom key) is never mangled.
pub fn strip_release_date(model: &str) -> &str {
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
pub fn pricing_file() -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("RALPHY_PRICING_FILE") {
        return Some(PathBuf::from(file));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".ralphy").join("pricing.toml"))
}

/// Resolve the optional models.dev disk cache: `$RALPHY_PRICING_CACHE` when set,
/// else `<home>/.ralphy/pricing-cache/models-dev.json`.
pub fn pricing_cache_file() -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("RALPHY_PRICING_CACHE") {
        return Some(PathBuf::from(file));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(
        PathBuf::from(home)
            .join(".ralphy")
            .join("pricing-cache")
            .join("models-dev.json"),
    )
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
mod tests;
