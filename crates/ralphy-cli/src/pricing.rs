//! The read-time price table (ADR-0008 D2/D8, ADR-0034 slice A). Tokens are the
//! immutable truth in the ledger; USD is a *projection* applied here at
//! read-time and never written. The floor is an embedded models.dev-shaped seed
//! plus a bare-id slug overlay; an optional disk cache and `pricing.toml`
//! override both. A model absent from every layer reports **unknown** cost
//! (`None`) — never `0`, which would hide spend.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use tracing::warn;

use ralphy_core::Usage;

pub(crate) mod fetch;
mod floor;
mod ingest;

/// The per-1M-token USD price for one model (ADR-0008 D8). The four fields mirror
/// [`Usage`]'s numeric split so each token kind is priced at its own rate — cache
/// reads in particular are ~1/10th of fresh input, so collapsing them would
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
    #[allow(dead_code)]
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
    pub(crate) fn from_layers(
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

    /// Resolve a model id through override → cache → seed → overlay, with the
    /// existing release-date / dots-to-dashes / provider-prefix normalizations
    /// applied to bare candidates, and provider-prefix synthesis for catalog keys.
    fn resolve(&self, model: &str) -> Option<&ModelPrice> {
        let candidates = bare_candidates(model);

        for c in &candidates {
            if let Some(p) = self.overrides.get(c.as_str()) {
                return Some(p);
            }
        }

        for c in &candidates {
            if let Some(key) = synthesize(c) {
                if let Some(p) = self.cache.get(&key) {
                    return Some(p);
                }
            }
            if let Some(p) = self.cache.get(c.as_str()) {
                return Some(p);
            }
        }

        for c in &candidates {
            if let Some(key) = synthesize(c) {
                if let Some(p) = self.seed.get(&key) {
                    return Some(p);
                }
            }
        }

        for c in &candidates {
            if let Some(p) = self.overlay.get(c.as_str()) {
                return Some(p);
            }
        }

        None
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
                warn!(path = %path.display(), error = %e, "parsing pricing.toml failed — using defaults")
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
        Err(flatten_err) => {
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
                    match v.try_into::<ModelPrice>() {
                        Ok(price) => {
                            overrides.insert(k, price);
                        }
                        Err(e) => {
                            return Err(format!("override `{k}`: {e}; flatten: {flatten_err}"));
                        }
                    }
                }
            }
            Ok(PricingTomlFile { offline, overrides })
        }
    }
}

/// `offline = true` from the resolved `pricing.toml`, or `false` when missing /
/// unreadable / malformed.
pub(crate) fn pricing_offline_from_file() -> bool {
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
pub(crate) fn synthesize(id: &str) -> Option<String> {
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
pub(crate) fn strip_release_date(model: &str) -> &str {
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
pub(crate) fn pricing_file() -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("RALPHY_PRICING_FILE") {
        return Some(PathBuf::from(file));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".ralphy").join("pricing.toml"))
}

/// Resolve the optional models.dev disk cache: `$RALPHY_PRICING_CACHE` when set,
/// else `<home>/.ralphy/pricing-cache/models-dev.json`.
pub(crate) fn pricing_cache_file() -> Option<PathBuf> {
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
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialises tests that mutate the process-global pricing env vars, so
    /// `cargo test`'s parallel runner can't race them (mirrors telegram config).
    pub(super) static ENV_LOCK: Mutex<()> = Mutex::new(());

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
    fn cost_usd_arithmetic_frozen_opus_fixture() {
        // ADR-0008 D8 oracle — inline rates only; no seed involved.
        let price = ModelPrice {
            input: 15.0,
            output: 75.0,
            cache_read: 1.5,
            cache_creation: 18.75,
        };
        let table = PriceTable::from_layers(
            BTreeMap::from([("claude-opus-4-8".into(), price)]),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        let cost = table
            .cost_usd("claude-opus-4-8", &one_million_each())
            .expect("fixture prices");
        assert!(
            (cost - 110.25).abs() < 1e-9,
            "frozen arithmetic must be 110.25, got {cost}"
        );
    }

    #[test]
    fn opus_pipeline_resolves_via_synthesis_to_seed() {
        let table = PriceTable::defaults();
        let cost = table
            .cost_usd("claude-opus-4-8", &one_million_each())
            .expect("opus resolves via synthesis to seed");
        // Seed carries the former defaults rates (15/75/1.5/18.75).
        assert!(
            (cost - 110.25).abs() < 1e-9,
            "pipeline cost tracks seed rates; got {cost}"
        );
        assert_eq!(
            synthesize("claude-opus-4-8").as_deref(),
            Some("anthropic/claude-opus-4-8")
        );
        assert!(table.seed.contains_key("anthropic/claude-opus-4-8"));
        assert!(!table.overlay.contains_key("claude-opus-4-8"));
    }

    #[test]
    fn resolver_seed_overlay_override_and_unknown() {
        let seed = BTreeMap::from([(
            "anthropic/claude-opus-4-8".into(),
            ModelPrice {
                input: 15.0,
                output: 75.0,
                cache_read: 1.5,
                cache_creation: 18.75,
            },
        )]);
        let overlay = BTreeMap::from([(
            "k2p6".into(),
            ModelPrice {
                input: 0.95,
                output: 4.0,
                cache_read: 0.16,
                cache_creation: 0.95,
            },
        )]);
        let table = PriceTable::from_layers(
            BTreeMap::new(),
            BTreeMap::new(),
            seed.clone(),
            overlay.clone(),
        );
        let tokens = one_million_each();

        let opus = table
            .cost_usd("claude-opus-4-8", &tokens)
            .expect("opus via seed synthesis");
        assert!((opus - 110.25).abs() < 1e-9);

        let k2 = table.cost_usd("k2p6", &tokens).expect("k2p6 via overlay");
        assert!((k2 - (0.95 + 4.0 + 0.16 + 0.95)).abs() < 1e-9);
        assert_eq!(synthesize("k2p6"), None);

        // Seed alone, no overlay row for opus; overlay alone no seed for k2p6.
        let seed_only =
            PriceTable::from_layers(BTreeMap::new(), BTreeMap::new(), seed, BTreeMap::new());
        assert!(seed_only.cost_usd("k2p6", &tokens).is_none());
        let overlay_only =
            PriceTable::from_layers(BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), overlay);
        assert!(overlay_only.cost_usd("claude-opus-4-8", &tokens).is_none());

        assert_eq!(table.cost_usd("big-pickle", &tokens), None);

        let overrides = BTreeMap::from([(
            "claude-opus-4-8".into(),
            ModelPrice {
                input: 30.0,
                output: 75.0,
                cache_read: 1.5,
                cache_creation: 18.75,
            },
        )]);
        let with_override = PriceTable::from_layers(
            overrides,
            BTreeMap::new(),
            BTreeMap::from([(
                "anthropic/claude-opus-4-8".into(),
                ModelPrice {
                    input: 15.0,
                    output: 75.0,
                    cache_read: 1.5,
                    cache_creation: 18.75,
                },
            )]),
            BTreeMap::new(),
        );
        let overridden = with_override
            .cost_usd("claude-opus-4-8", &tokens)
            .expect("override");
        assert!(
            (overridden - 125.25).abs() < 1e-9,
            "override must beat seed; got {overridden}"
        );
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

    #[test]
    fn offline_true_with_model_override_still_loads_overrides() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir =
            std::env::temp_dir().join(format!("ralphy-pricing-offline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("pricing.toml");
        std::fs::write(
            &file,
            "offline = true\n\n[claude-opus-4-8]\ninput = 30.0\noutput = 75.0\ncache_read = 1.5\ncache_creation = 18.75\n",
        )
        .expect("write");
        std::env::set_var("RALPHY_PRICING_FILE", &file);
        assert!(pricing_offline_from_file());
        let loaded = PriceTable::load();
        let cost = loaded
            .cost_usd("claude-opus-4-8", &one_million_each())
            .unwrap();
        assert!(
            (cost - 125.25).abs() < 1e-9,
            "offline + override must still reprice; got {cost}"
        );
        std::env::remove_var("RALPHY_PRICING_FILE");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_model_never_returns_some_zero() {
        let table = PriceTable::defaults();
        assert_eq!(table.cost_usd("big-pickle", &one_million_each()), None);
    }

    #[test]
    fn synthesize_maps_each_provider_prefix() {
        assert_eq!(
            synthesize("claude-opus-4-8").as_deref(),
            Some("anthropic/claude-opus-4-8")
        );
        assert_eq!(synthesize("gpt-5.5").as_deref(), Some("openai/gpt-5.5"));
        assert_eq!(
            synthesize("gemini-3-flash").as_deref(),
            Some("google/gemini-3-flash")
        );
        assert_eq!(
            synthesize("kimi-for-coding").as_deref(),
            Some("moonshotai/kimi-for-coding")
        );
        assert_eq!(synthesize("k2p6"), None);
        assert_eq!(synthesize("composer-2.5"), None);
    }

    #[test]
    fn cache_layer_beats_seed_via_synthesis() {
        let seed = BTreeMap::from([(
            "anthropic/claude-opus-4-8".into(),
            ModelPrice {
                input: 15.0,
                output: 75.0,
                cache_read: 1.5,
                cache_creation: 18.75,
            },
        )]);
        let cache = BTreeMap::from([(
            "anthropic/claude-opus-4-8".into(),
            ModelPrice {
                input: 5.0,
                output: 25.0,
                cache_read: 0.5,
                cache_creation: 6.25,
            },
        )]);
        let table = PriceTable::from_layers(BTreeMap::new(), cache, seed, BTreeMap::new());
        let cost = table
            .cost_usd("claude-opus-4-8", &one_million_each())
            .expect("cache hit");
        // Cache rates 5+25+0.5+6.25 = 36.75, not seed's 110.25.
        assert!(
            (cost - 36.75).abs() < 1e-9,
            "cache must beat seed; got {cost}"
        );
    }

    #[test]
    fn load_reads_disk_cache_when_ralphy_pricing_cache_set() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("ralphy-pricing-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let cache_file = dir.join("models-dev.json");
        std::fs::write(
            &cache_file,
            r#"{"timestamp":"2026-07-23T00:00:00Z","data":{"anthropic/claude-opus-4-8":{"input":5.0,"output":25.0,"cache_read":0.5,"cache_creation":6.25}}}"#,
        )
        .expect("write cache");
        std::env::set_var("RALPHY_PRICING_CACHE", &cache_file);
        // Ensure no override file interferes.
        std::env::set_var(
            "RALPHY_PRICING_FILE",
            dir.join("missing-pricing.toml").as_os_str(),
        );

        let loaded = PriceTable::load();
        let cost = loaded
            .cost_usd("claude-opus-4-8", &one_million_each())
            .expect("cache-backed opus");
        assert!(
            (cost - 36.75).abs() < 1e-9,
            "load must apply disk cache over seed; got {cost}"
        );

        std::env::remove_var("RALPHY_PRICING_CACHE");
        std::env::remove_var("RALPHY_PRICING_FILE");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
