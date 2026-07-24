//! Parsing Gemini's `result.stats` envelope into normalized [`Usage`] records
//! (ADR-0043 D9). Three arithmetic traps the spike measured:
//! 1. billable output is `total - input`, not the `output_tokens` field —
//!    the field excludes thinking tokens, which bill at the output rate (a
//!    25x under-count observed on a real run).
//! 2. `input_tokens` already contains `cached`; adding the two double-counts.
//! 3. a multi-model run reports per-model figures, including the routing
//!    model's own consumption, rather than collapsing to one engine.

use ralphy_core::Usage;
use serde_json::Value;

use crate::model::price_key;

/// Parse one call's `result.stats` object into per-model [`Usage`] records.
/// Prefers `stats.models` — each key folded through [`price_key`] (D8), the
/// only place the routing model's own consumption appears separately — and
/// falls back to the flattened top-level fields with `model: None` when
/// `models` is absent or empty.
pub(crate) fn parse_stream_stats(stats: &Value) -> Vec<Usage> {
    match stats.get("models").and_then(Value::as_object) {
        Some(map) if !map.is_empty() => map
            .iter()
            .map(|(id, record)| record_usage(record, Some(price_key(id))))
            .collect(),
        _ => vec![record_usage(stats, None)],
    }
}

/// One `stats` (or `stats.models.<id>`) record reduced to a [`Usage`].
fn record_usage(record: &Value, model: Option<String>) -> Usage {
    let field = |k: &str| record.get(k).and_then(Value::as_u64).unwrap_or(0);
    let input_tokens = field("input_tokens");
    let cached = field("cached");
    let input = match record.get("input").and_then(Value::as_u64) {
        Some(v) => v,
        None => input_tokens.saturating_sub(cached),
    };
    let output = match record.get("total_tokens").and_then(Value::as_u64) {
        Some(total) => total.saturating_sub(input_tokens),
        None => field("output_tokens"),
    };
    Usage {
        input,
        output,
        cache_read: cached,
        cache_creation: 0,
        model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one real capture (`.ralphy/gemprobe/out.jsonl`, 2026-07-21, an
    /// errored run) — usage is parsed independent of `status`.
    const LIVE: &str = include_str!("../fixtures/usage-live-2026-07-21.jsonl");
    /// Hand-written from the spike's measured figures (§2, consequences 1 and
    /// 2b): the two model rows sum exactly to the top-level fields, and the
    /// field NAMES are cross-checked against `LIVE`.
    const CACHED_MULTIMODEL: &str =
        include_str!("../fixtures/usage-cached-multimodel-2026-07-21.jsonl");

    fn stats_from_result_line(jsonl: &str) -> Value {
        jsonl
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
            .find_map(|v| {
                (v.get("type").and_then(Value::as_str) == Some("result"))
                    .then(|| v.get("stats").cloned())
                    .flatten()
            })
            .expect("fixture must carry a result record with stats")
    }

    #[test]
    fn billable_output_is_total_minus_input_not_the_output_field() {
        let stats = stats_from_result_line(CACHED_MULTIMODEL);
        let folded = Usage::fold_usage(&parse_stream_stats(&stats), None);
        assert_eq!(folded.output, 2208);
        assert_ne!(
            folded.output, 88,
            "the raw output_tokens field, not the derived total"
        );
    }

    #[test]
    fn cached_input_is_counted_once() {
        let stats = stats_from_result_line(CACHED_MULTIMODEL);
        let folded = Usage::fold_usage(&parse_stream_stats(&stats), None);
        assert_eq!(folded.input, 48628);
        assert_eq!(folded.cache_read, 16273);
        assert_eq!(folded.input + folded.cache_read, 64901);
    }

    #[test]
    fn a_multi_model_run_keeps_every_engines_tokens() {
        let stats = stats_from_result_line(CACHED_MULTIMODEL);
        let items = parse_stream_stats(&stats);
        assert_eq!(items.len(), 2);
        let folded = Usage::fold_usage(&items, None);
        assert_eq!(folded.total(), 67109);
        assert_eq!(folded.model.as_deref(), Some("gemini-3.1-pro-preview"));
        // The routing model's own consumption is included, not dropped: the
        // folded total is more than the heaviest single model's own total.
        assert_ne!(folded.total(), 65609);
    }

    #[test]
    fn the_live_envelope_is_read_from_result_stats() {
        let stats = stats_from_result_line(LIVE);
        let folded = Usage::fold_usage(&parse_stream_stats(&stats), None);
        assert_eq!(folded.input, 868);
        assert_eq!(folded.output, 632);
        assert_eq!(folded.model.as_deref(), Some("gemini-3.1-flash-lite"));
    }

    /// No cache-creation counter exists on this vendor (D9 trap 2) — fixed at
    /// zero, never left to a hopeful field lookup.
    #[test]
    fn cache_creation_is_always_zero() {
        let stats = stats_from_result_line(CACHED_MULTIMODEL);
        for item in parse_stream_stats(&stats) {
            assert_eq!(item.cache_creation, 0);
        }
    }

    /// An empty `stats` object (no fields at all, so no `models` key either)
    /// still returns exactly one zero-valued record, never an empty `Vec` —
    /// the fallback arm always pushes one item. This is what makes
    /// `GeminiFold.usage`'s `None` vs `Some` split meaningful: `Some` is
    /// never the genuinely-empty state a naive reading of "zero usage" might
    /// expect (self-review #263 finding).
    #[test]
    fn an_empty_stats_object_still_returns_one_record_not_an_empty_vec() {
        let items = parse_stream_stats(&serde_json::json!({}));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0], Usage::default());
    }

    /// A record with no `models` key falls back to the flattened top-level
    /// fields, attributed `model: None` so `fold_usage`'s fallback resolves it
    /// to the requested model's price key instead.
    #[test]
    fn a_record_without_models_falls_back_to_the_top_level_fields() {
        let stats = serde_json::json!({
            "total_tokens": 100,
            "input_tokens": 60,
            "output_tokens": 5,
            "cached": 10,
            "input": 50
        });
        let items = parse_stream_stats(&stats);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].model, None);
        assert_eq!(items[0].input, 50);
        assert_eq!(items[0].output, 40);
        assert_eq!(items[0].cache_read, 10);
    }
}
