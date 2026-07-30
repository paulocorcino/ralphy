//! Embedded seed ⊕ slug-overlay floor (ADR-0034 A3). Rates in the seed are a
//! snapshot of the former `defaults.rs` table — not live models.dev (which
//! currently lists opus lower than ADR-0008 D8). `#290` may later refresh majors.

use std::collections::BTreeMap;

use serde_json::Value;

use super::ingest::ingest_models_dev;
use super::{ModelPrice, PriceTable};

const SEED_JSON: &str = include_str!("../../../assets/pricing/models-dev-seed.json");
const OVERLAY_JSON: &str = include_str!("../../../assets/pricing/slug-overlay.json");

impl PriceTable {
    /// The shipped floor: ingested seed (`provider/model`) plus bare-id overlay.
    /// No overrides, no disk cache — that is [`PriceTable::load`].
    pub fn defaults() -> Self {
        let seed_doc: Value =
            serde_json::from_str(SEED_JSON).expect("embedded models-dev-seed.json must parse");
        let seed = ingest_models_dev(&seed_doc);
        let overlay: BTreeMap<String, ModelPrice> =
            serde_json::from_str(OVERLAY_JSON).expect("embedded slug-overlay.json must parse");
        Self::from_layers(BTreeMap::new(), BTreeMap::new(), seed, overlay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::one_million_each;

    /// Golden lock: every bare id that lived in the retired `defaults.rs` still
    /// prices to the same 1M-each USD via seed ⊕ overlay (issue #288 AC1).
    ///
    /// A row whose seed cost is refreshed from upstream moves its expected value
    /// here in the same change (ADR-0034 A6 (b)) — the lock guards the resolution
    /// path, not the mutable opinion that is a price. Rows that carry a
    /// *deliberate* floor above upstream (`claude-opus-4-8`, ADR-0008 D8) must NOT
    /// move: the refresh job's PR restores them by hand.
    #[test]
    fn every_former_defaults_id_prices_identically_from_seed_and_overlay() {
        let table = PriceTable::defaults();
        let tokens = one_million_each();
        // (bare id, expected USD over 1M of each token kind)
        let rows: &[(&str, f64)] = &[
            ("claude-opus-4-8", 110.25),
            ("claude-sonnet-4-6", 22.05),
            ("claude-haiku-4-5", 7.35),
            ("gpt-5.5", 40.5),
            ("k2p6", 6.06),
            ("kimi-for-coding", 6.06),
            ("k3", 6.06),
            // Refreshed to upstream's published 2/10/0.2/2.5; it had carried
            // sonnet-4-6's rate, which no decision ever pinned it to.
            ("claude-sonnet-5", 14.7),
            ("kimi-k2.7-code", 6.06),
            ("auto", 10.5),
            ("composer-2.5", 3.7),
            ("cursor-grok-4.5", 10.5),
            ("glm-5.2", 7.46),
            ("gemini-3-flash", 4.05),
            ("gemini-3.1-pro", 16.2),
            ("gemini-3.5-flash", 12.15),
            ("gpt-5.6-sol", 41.75),
            ("gpt-5.6-terra", 20.875),
            ("gpt-5.6-luna", 8.35),
            ("gpt-5.1", 12.625),
            ("gpt-5.2", 17.675),
            ("gpt-5.3-codex", 17.675),
            ("gpt-5.4", 20.25),
            ("gpt-5.4-mini", 6.075),
            ("gpt-5.4-nano", 1.67),
            ("claude-opus-4-7", 36.75),
            ("claude-fable-5", 73.5),
            ("claude-4.6-sonnet", 22.05),
            ("claude-4.6-opus", 36.75),
            ("claude-4.5-sonnet", 22.05),
            ("claude-4.5-haiku", 7.35),
            ("claude-4.5-opus", 36.75),
            ("claude-4-sonnet", 22.05),
            ("gpt-5-mini", 6.075),
            ("gemini-3.1-pro-preview", 16.2),
            ("gemini-3-flash-preview", 4.05),
            ("gemini-3.1-flash-lite", 2.025),
            ("gemini-2.5-pro", 12.625),
            ("gemini-2.5-flash", 3.13),
        ];
        assert_eq!(rows.len(), 39, "former defaults.rs had 39 priced ids");
        for &(id, expected) in rows {
            let got = table
                .cost_usd(id, &tokens)
                .unwrap_or_else(|| panic!("{id} must still price from seed⊕overlay"));
            assert!(
                (got - expected).abs() < 1e-9,
                "{id}: expected {expected}, got {got}"
            );
        }
    }

    #[test]
    fn cross_vendor_codex_and_opencode_ids_resolve_to_a_price() {
        // The exact ids the Codex and OpenCode adapters emit (`gpt-5.5`, `k2p6`)
        // must resolve in the floor, or every cross-vendor run reports `~$?`.
        let table = PriceTable::defaults();
        let tokens = one_million_each();
        assert!(
            table.cost_usd("gpt-5.5", &tokens).is_some(),
            "Codex's `gpt-5.5` must be priced by the floor"
        );
        assert!(
            table.cost_usd("k2p6", &tokens).is_some(),
            "OpenCode's `k2p6` must be priced by the floor"
        );
        // Both Kimi surfaces must price: the run path's PREFIXED id (via `resolve`'s
        // provider-prefix fallback) and the usage scan's BARE id (exact key).
        assert!(
            table
                .cost_usd("kimi-code/kimi-for-coding", &tokens)
                .is_some(),
            "the Kimi run path's prefixed `kimi-code/kimi-for-coding` must price (ADR-0028)"
        );
        assert!(
            table.cost_usd("kimi-for-coding", &tokens).is_some(),
            "the usage scan's bare `kimi-for-coding` must price (ADR-0028)"
        );
        assert!(
            table.cost_usd("kimi-code/k3", &tokens).is_some(),
            "the 0.28 Kimi run path's prefixed `kimi-code/k3` must price (ADR-0028 D4)"
        );
        assert!(
            table.cost_usd("k3", &tokens).is_some(),
            "the 0.28 usage scan's bare `k3` must price — the #274 gap (ADR-0028 D4)"
        );
    }

    /// The Claude adapter's own current majors must price. `claude-opus-5` was
    /// missing from the seed while its sibling `claude-sonnet-5` was present, so
    /// every opus run reported `$?` and logged "add `claude-opus-5` to pricing.toml".
    /// Rates are models.dev's published Anthropic table (5/25/0.5/6.25).
    #[test]
    fn current_claude_majors_resolve_to_a_price() {
        let table = PriceTable::defaults();
        let tokens = one_million_each();
        let opus5 = table
            .cost_usd("claude-opus-5", &tokens)
            .expect("the Claude adapter's current opus must price");
        assert!(
            (opus5 - (5.0 + 25.0 + 0.5 + 6.25)).abs() < 1e-9,
            "claude-opus-5 priced field-by-field; got {opus5}"
        );
        assert!(
            table.cost_usd("claude-sonnet-5", &tokens).is_some(),
            "its sonnet sibling must stay priced"
        );
    }

    #[test]
    fn copilot_model_ids_resolve_to_a_price() {
        let table = PriceTable::defaults();
        let tokens = one_million_each();
        assert!(
            table.cost_usd("claude-sonnet-5", &tokens).is_some(),
            "Copilot's account-default `claude-sonnet-5` must be priced"
        );
        let kimi = table
            .cost_usd("kimi-k2.7-code", &tokens)
            .expect("Copilot's `kimi-k2.7-code` must be priced");
        assert!(
            (kimi - (0.95 + 4.0 + 0.16 + 0.95)).abs() < 1e-9,
            "kimi-k2.7-code priced field-by-field; got {kimi}"
        );
        let dotted = table
            .cost_usd("claude-haiku-4.5", &tokens)
            .expect("the dotted Anthropic id resolves via dot→dash");
        let dashed = table.cost_usd("claude-haiku-4-5", &tokens).unwrap();
        assert!(
            (dotted - dashed).abs() < 1e-9,
            "dotted and dashed forms must price identically: {dotted} vs {dashed}"
        );
        assert!(
            table.cost_usd("zzz-not.real", &tokens).is_none(),
            "normalization must not price a genuinely unknown model"
        );
    }
}
