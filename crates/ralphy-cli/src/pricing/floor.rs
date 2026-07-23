//! Embedded seed ⊕ slug-overlay floor (ADR-0034 A3). Rates in the seed are a
//! snapshot of the former `defaults.rs` table — not live models.dev (which
//! currently lists opus lower than ADR-0008 D8). `#290` may later refresh majors.

use std::collections::BTreeMap;

use serde_json::Value;

use super::ingest::ingest_models_dev;
use super::{ModelPrice, PriceTable};

const SEED_JSON: &str = include_str!("../../../../assets/pricing/models-dev-seed.json");
const OVERLAY_JSON: &str = include_str!("../../../../assets/pricing/slug-overlay.json");

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
    use crate::pricing::tests::one_million_each;

    /// The Gemini axis end to end (ADR-0043 D8): the lookup goes through the
    /// adapter's own `price_key`, so the table and the vendor's id grammar cannot
    /// drift apart — and the two ids that collide with a Cursor row of the same
    /// spelling stay un-conflated.
    #[test]
    fn gemini_ids_price_through_the_adapters_key() {
        let table = PriceTable::defaults();
        let t = one_million_each();
        let cost = |key: &str| table.cost_usd(key, &t);

        // The 3× trap: the CLI's `gemini-3-flash` is served by the 3.5 backend…
        let cli_flash = cost(&ralphy_agent_gemini::price_key("gemini-3-flash"))
            .expect("the CLI's flash must price");
        assert!((cli_flash - 12.15).abs() < 1e-9, "got {cli_flash}");
        // …while the raw row of that spelling is Cursor's preview Flash.
        let cursor_flash = cost("gemini-3-flash").expect("Cursor's row must survive");
        assert!((cursor_flash - 4.05).abs() < 1e-9, "got {cursor_flash}");
        assert!(
            cli_flash > cursor_flash,
            "the two must stay distinct rows, not one conflated price"
        );

        // The routing model the CLI actually dispatches to is priced.
        let lite = cost("gemini-3.1-flash-lite").expect("the routing model must price");
        assert!((lite - 2.025).abs() < 1e-9, "got {lite}");

        // No published price ⇒ no row: unpriced beats guessed. `cost_usd` reports
        // `None`, which the report renders as `~$?`, never `0`.
        assert_eq!(cost("gemini-3.1-pro-preview-customtools"), None);
        // And a routed run never borrows another vendor's `auto` row.
        assert_eq!(cost(&ralphy_agent_gemini::price_key("auto")), None);
        assert!(
            cost("auto").is_some(),
            "Cursor's own `auto` row must be untouched"
        );

        // Retired for pinning, still priced — as its successor.
        let retired = cost(&ralphy_agent_gemini::price_key("gemini-3-pro-preview"));
        assert!(retired.is_some(), "a historical run record must cost out");
        assert_eq!(retired, cost("gemini-3.1-pro-preview"));
    }

    /// The Cursor axis end to end: the adapter's own normalizer feeds the lookup,
    /// so the price key and the vendor's id grammar can never drift apart.
    #[test]
    fn cursor_families_resolve_to_a_price() {
        let table = PriceTable::defaults();
        let tokens = one_million_each();
        for id in [
            "composer-2.5-fast",
            "auto",
            "cursor-grok-4.5-low",
            "glm-5.2-high",
            "gpt-5.6-sol-max",
            "gemini-3-flash",
            "claude-opus-4-8[context=1m,effort=high,fast=false]",
            // An unknown EFFORT must not make a known family unknown.
            "composer-2.5-xhigh",
        ] {
            let family = ralphy_agent_cursor::model_family(id);
            assert!(
                table.cost_usd(&family, &tokens).is_some(),
                "{id} normalized to {family}, which the floor does not price"
            );
        }
        // An exact oracle on one row: `is_some()` alone stays green with
        // `cache_read` and `cache_creation` transposed.
        let composer = table
            .cost_usd(
                &ralphy_agent_cursor::model_family("composer-2.5-fast"),
                &tokens,
            )
            .expect("composer is priced");
        assert!(
            (composer - (0.5 + 2.5 + 0.2 + 0.5)).abs() < 1e-9,
            "composer-2.5 priced field-by-field; got {composer}"
        );
        // An unknown FAMILY still logs an unknown model.
        assert_eq!(
            table.cost_usd(
                &ralphy_agent_cursor::model_family("definitely-not-a-real-model-high"),
                &tokens
            ),
            None
        );
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
