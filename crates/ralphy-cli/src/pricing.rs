//! Thin shim over the [`ralphy_pricing`] crate (ADR-0034 D6): pricing itself
//! lives there, carrying no `ralphy-core` or adapter edge so the daemon can
//! depend on it too. What stays here is the conversion at this crate's own
//! boundary — [`ralphy_core::Usage`] into [`TokenCounts`] — plus the re-exports
//! that keep every existing `crate::pricing::…` path resolving.

use std::collections::BTreeMap;

use ralphy_core::Usage;

pub(crate) use ralphy_pricing::{
    fetch, pricing_cache_file, pricing_offline_from_file, PriceTable, TokenCounts,
};

/// One usage record's four token counts, dropping the model id the price table
/// takes as its separate `&str` argument. A free fn rather than a `From` impl:
/// both types are foreign here, so the orphan rule forbids the impl, and giving
/// `ralphy-core` the edge instead would point the dependency arrow outward
/// (ADR-0002).
pub(crate) fn counts(u: &Usage) -> TokenCounts {
    TokenCounts {
        input: u.input,
        output: u.output,
        cache_read: u.cache_read,
        cache_creation: u.cache_creation,
    }
}

/// [`counts`] across a per-model split, for [`PriceTable::cost_usd_by_model`].
pub(crate) fn counts_by_model(by_model: &BTreeMap<String, Usage>) -> BTreeMap<String, TokenCounts> {
    by_model
        .iter()
        .map(|(m, u)| (m.clone(), counts(u)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_million_each() -> TokenCounts {
        TokenCounts {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_creation: 1_000_000,
        }
    }

    /// `counts` must carry each kind into its own slot — a transposition here
    /// would misprice cache reads at fresh-input rates.
    #[test]
    fn counts_maps_each_field_to_its_own_slot() {
        let u = Usage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_creation: 4,
            model: Some("claude-opus-4-8".into()),
        };
        assert_eq!(
            counts(&u),
            TokenCounts {
                input: 1,
                output: 2,
                cache_read: 3,
                cache_creation: 4,
            }
        );
        // The bucket key and `Usage.model` are DELIBERATELY different here: the
        // real callers bucket an unattributed record under `unknown` while its
        // `model` is `None`, so keying the output off `u.model` would be wrong.
        let by_model = BTreeMap::from([
            (
                "unknown".to_string(),
                Usage {
                    model: None,
                    ..u.clone()
                },
            ),
            ("the-bucket-key".to_string(), u),
        ]);
        let out = counts_by_model(&by_model);
        assert_eq!(
            out.keys().collect::<Vec<_>>(),
            vec!["the-bucket-key", "unknown"],
            "the caller's bucket keys must survive, not the records' own model ids"
        );
        assert_eq!(
            out.get("the-bucket-key"),
            Some(&TokenCounts {
                input: 1,
                output: 2,
                cache_read: 3,
                cache_creation: 4,
            })
        );
        assert_eq!(out.get("unknown").map(|t| t.total()), Some(10));
    }

    /// The tokio/reqwest absence pin that guarded `ralphy-cli` before the pricing
    /// extraction moved its host file out (ADR-0034 A6, ADR-0032 §10: async stays
    /// confined to the daemon). `ralphy-pricing` has its own copy; this is the CLI's.
    #[test]
    fn cli_manifest_pins_ureq_excludes_reqwest_tokio() {
        let manifest = include_str!("../Cargo.toml");
        assert!(manifest.contains("ureq"), "ralphy-cli must depend on ureq");
        // Build needles from parts so this file cannot trip an absence pin on itself.
        let reqwest = ["req", "west"].concat();
        let tokio = ["tok", "io"].concat();
        assert!(
            !manifest.contains(&reqwest),
            "ralphy-cli must not depend on {reqwest}"
        );
        assert!(
            !manifest.contains(&tokio),
            "ralphy-cli must not depend on {tokio}"
        );
    }

    #[test]
    fn refresh_if_stale_sole_production_call_is_usage_cmd() {
        // Concatenate so include_str of this file cannot match the needle via its
        // own source text describing the pin.
        let name = ["refresh_if_", "stale"].concat();
        let usage = include_str!("usage.rs");
        let report = include_str!("run/report.rs");
        let presenter = include_str!("ui/presenter.rs");
        let pricing_root = include_str!("../../ralphy-pricing/src/lib.rs");
        let floor = include_str!("../../ralphy-pricing/src/floor.rs");
        let ingest = include_str!("../../ralphy-pricing/src/ingest.rs");

        let usage_hits = usage.matches(&name).count();
        assert!(usage_hits >= 1, "usage.rs must call {name}");
        assert_eq!(
            report.matches(&name).count(),
            0,
            "run/report.rs must not call {name}"
        );
        assert_eq!(
            presenter.matches(&name).count(),
            0,
            "ui/presenter.rs must not call {name}"
        );
        assert_eq!(
            pricing_root.matches(&name).count(),
            0,
            "pricing lib.rs must not call {name}"
        );
        assert_eq!(floor.matches(&name).count(), 0);
        assert_eq!(ingest.matches(&name).count(), 0);
    }

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
}
