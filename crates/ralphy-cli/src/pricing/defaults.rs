//! The shipped default price table, split out of `pricing.rs` under ADR-0022:
//! the rows and the tests that assert them are one responsibility, and the file
//! was at its 500-line limit. `PriceTable::defaults` is an inherent method, so it
//! stays reachable at `crate::pricing::PriceTable` — no `pub` item moved.

use std::collections::BTreeMap;

use super::{ModelPrice, PriceTable};

impl PriceTable {
    /// The shipped defaults for the models actually in use. `claude-opus-4-8` is
    /// pinned from ADR-0008 D8 (the canonical test oracle); the sonnet/haiku and
    /// the cross-vendor `gpt-5.5`/`k2p6` entries are indicative current list prices
    /// and are not asserted by tests.
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
        // Codex (OpenAI) and OpenCode (Moonshot) models actually in use — indicative
        // list prices captured 2026-06, not asserted by tests. Neither provider
        // charges a cache-write premium (context caching is automatic), so
        // `cache_creation` is priced at the plain input rate — unlike Anthropic's
        // 1.25× cache writes above. Keyed on the exact id each adapter reports
        // (`gpt-5.5` from Codex, `k2p6` from OpenCode), so they resolve directly.
        t.insert(
            "gpt-5.5".to_string(),
            ModelPrice {
                input: 5.0,
                output: 30.0,
                cache_read: 0.5,
                cache_creation: 5.0,
            },
        );
        // `k2p6` is OpenCode's id for Moonshot's Kimi K2.6 flagship.
        t.insert(
            "k2p6".to_string(),
            ModelPrice {
                input: 0.95,
                output: 4.0,
                cache_read: 0.16,
                cache_creation: 0.95,
            },
        );
        // `kimi-code/kimi-for-coding` is the id the native Kimi adapter reports
        // (ADR-0028 D4; "K2.7 Code"). Priced with the same indicative K2-family
        // list prices as `k2p6` so a `--agent kimi` run costs out instead of
        // logging "unknown model"; Moonshot bills no separate cache-write premium,
        // so `cache_creation` matches the plain input rate.
        t.insert(
            "kimi-code/kimi-for-coding".to_string(),
            ModelPrice {
                input: 0.95,
                output: 4.0,
                cache_read: 0.16,
                cache_creation: 0.95,
            },
        );
        // `kimi-code/k3` is the id kimi-code 0.28 reports (ADR-0028 D4); the row
        // above stays for runs recorded before the 0.28 cut. Same indicative
        // K2-family rates.
        t.insert(
            "kimi-code/k3".to_string(),
            ModelPrice {
                input: 0.95,
                output: 4.0,
                cache_read: 0.16,
                cache_creation: 0.95,
            },
        );
        // Copilot's catalog ids (ADR-0041 D10). Copilot bills in AI CREDITS, not
        // tokens; there is no documented nano-AIU→USD rate, so these rows price the
        // rows at the UNDERLYING vendor's list price — ADR-0034's counterfactual
        // "what would this have cost on metered API". Indicative, not asserted.
        // The Anthropic ids Copilot spells with a dot (`claude-haiku-4.5`) need no
        // row: `resolve`'s dot→dash fallback reuses the family entries above.
        t.insert(
            "claude-sonnet-5".to_string(),
            ModelPrice {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_creation: 3.75,
            },
        );
        // Copilot's id for Moonshot's K2.7 Code — same K2-family figures as `k2p6`.
        t.insert(
            "kimi-k2.7-code".to_string(),
            ModelPrice {
                input: 0.95,
                output: 4.0,
                cache_read: 0.16,
                cache_creation: 0.95,
            },
        );
        // ── Cursor (ADR-0042 D5) ─────────────────────────────────────────────
        // Rates from cursor.com/docs/models, per 1M tokens, keyed by the family
        // `ralphy_agent_cursor::model_family` folds a pinned id onto. Three
        // caveats ride these rows:
        //  - Cursor bills in CREDITS, not tokens, so every figure here is
        //    ADR-0034's counterfactual "what the metered API would have cost".
        //  - Where Cursor publishes no cache-write column, `cache_creation`
        //    repeats the input rate — the convention `gpt-5.5`/`k2p6` already use.
        //  - `claude-opus-4-8` is NOT repriced: Cursor lists it at a third of
        //    Anthropic's list price, and that key is shared across vendors and
        //    pinned by ADR-0008 D8's oracle. A Cursor run pinning Opus therefore
        //    reports the metered-API counterfactual, not a Cursor bill.
        for (model, input, output, cache_read, cache_creation) in [
            // `auto` is the routed path, priced at the family the spike observed
            // it routing to (`cursor-grok-4.5`) so an unpinned run costs out
            // instead of logging an unknown model.
            ("auto", 2.0, 6.0, 0.5, 2.0),
            ("composer-2.5", 0.5, 2.5, 0.2, 0.5),
            ("cursor-grok-4.5", 2.0, 6.0, 0.5, 2.0),
            ("glm-5.2", 1.4, 4.4, 0.26, 1.4),
            ("gemini-3-flash", 0.5, 3.0, 0.05, 0.5),
            ("gemini-3.1-pro", 2.0, 12.0, 0.2, 2.0),
            ("gemini-3.5-flash", 1.5, 9.0, 0.15, 1.5),
            ("gpt-5.6-sol", 5.0, 30.0, 0.5, 6.25),
            ("gpt-5.6-terra", 2.5, 15.0, 0.25, 3.125),
            ("gpt-5.6-luna", 1.0, 6.0, 0.1, 1.25),
            ("gpt-5.1", 1.25, 10.0, 0.125, 1.25),
            ("gpt-5.2", 1.75, 14.0, 0.175, 1.75),
            ("gpt-5.3-codex", 1.75, 14.0, 0.175, 1.75),
            ("gpt-5.4", 2.5, 15.0, 0.25, 2.5),
            ("gpt-5.4-mini", 0.75, 4.5, 0.075, 0.75),
            ("gpt-5.4-nano", 0.2, 1.25, 0.02, 0.2),
            ("claude-opus-4-7", 5.0, 25.0, 0.5, 6.25),
            ("claude-fable-5", 10.0, 50.0, 1.0, 12.5),
            ("claude-4.6-sonnet", 3.0, 15.0, 0.3, 3.75),
            ("claude-4.6-opus", 5.0, 25.0, 0.5, 6.25),
            ("claude-4.5-sonnet", 3.0, 15.0, 0.3, 3.75),
            ("claude-4.5-haiku", 1.0, 5.0, 0.1, 1.25),
            // Reachable in the live catalogue but absent from the pricing page —
            // priced at their published sibling's rate so no reachable id logs an
            // unknown model. Indicative even by this table's standards.
            ("claude-4.5-opus", 5.0, 25.0, 0.5, 6.25),
            ("claude-4-sonnet", 3.0, 15.0, 0.3, 3.75),
            ("gpt-5-mini", 0.75, 4.5, 0.075, 0.75),
        ] {
            t.insert(
                model.to_string(),
                ModelPrice {
                    input,
                    output,
                    cache_read,
                    cache_creation,
                },
            );
        }
        PriceTable(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::tests::one_million_each;

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
                "{id} normalized to {family}, which the defaults do not price"
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
    fn cross_vendor_codex_and_opencode_ids_resolve_to_a_price() {
        // The exact ids the Codex and OpenCode adapters emit (`gpt-5.5`, `k2p6`)
        // must resolve in the defaults, or every cross-vendor run reports `~$?`.
        // This guards the key spelling, not the indicative figures themselves.
        let table = PriceTable::defaults();
        let tokens = one_million_each();
        assert!(
            table.cost_usd("gpt-5.5", &tokens).is_some(),
            "Codex's `gpt-5.5` must be priced by the defaults"
        );
        assert!(
            table.cost_usd("k2p6", &tokens).is_some(),
            "OpenCode's `k2p6` must be priced by the defaults"
        );
        assert!(
            table
                .cost_usd("kimi-code/kimi-for-coding", &tokens)
                .is_some(),
            "the native Kimi adapter's `kimi-code/kimi-for-coding` must be priced (ADR-0028)"
        );
        assert!(
            table.cost_usd("kimi-code/k3", &tokens).is_some(),
            "the 0.28 Kimi adapter's `kimi-code/k3` must be priced (ADR-0028 D4)"
        );
    }

    #[test]
    fn copilot_model_ids_resolve_to_a_price() {
        // The ids Copilot's catalog reports. `claude-haiku-4.5` differs from the
        // table's `claude-haiku-4-5` by punctuation only and must price identically
        // — but normalization must not turn an unknown dotted id into a price.
        let table = PriceTable::defaults();
        let tokens = one_million_each();
        assert!(
            table.cost_usd("claude-sonnet-5", &tokens).is_some(),
            "Copilot's account-default `claude-sonnet-5` must be priced"
        );
        // An exact oracle on one row: `is_some()` alone would stay green with
        // `cache_read` and `cache_creation` transposed, mispricing every run.
        // 1M of each field at 0.95 / 4.0 / 0.16 / 0.95.
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
