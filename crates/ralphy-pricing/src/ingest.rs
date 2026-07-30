//! models.dev → normalized `provider/model → ModelPrice` ingest (ADR-0034 A6).
//! Pure: no network, no disk. Shared by the embedded seed load and the fetch
//! path (`pricing::fetch`).

use std::collections::BTreeMap;

use serde_json::Value;

use super::{strip_release_date, ModelPrice};

/// Walk a models.dev-shaped document (`providers → models → cost`) into a
/// normalized table. Renames `cache_write` → `cache_creation`, maps null/missing
/// cache fields to `0.0`, and drops rows without a usable input+output cost (the
/// `$0`/subscription trap). Malformed entries are skipped; a non-object root
/// yields an empty map — never panics.
pub fn ingest_models_dev(doc: &Value) -> BTreeMap<String, ModelPrice> {
    let mut out = BTreeMap::new();
    let Some(providers) = doc.as_object() else {
        return out;
    };
    for (provider, pval) in providers {
        let Some(models) = pval.get("models").and_then(|m| m.as_object()) else {
            continue;
        };
        for (model_id, mval) in models {
            let Some(cost) = mval.get("cost") else {
                continue;
            };
            let Some(input) = json_f64(cost.get("input")) else {
                continue;
            };
            let Some(output) = json_f64(cost.get("output")) else {
                continue;
            };
            if input == 0.0 && output == 0.0 {
                continue;
            }
            let cache_read = json_f64(cost.get("cache_read")).unwrap_or(0.0);
            let cache_creation = json_f64(cost.get("cache_write")).unwrap_or(0.0);
            let key = format!("{provider}/{}", strip_release_date(model_id));
            out.insert(
                key,
                ModelPrice {
                    input,
                    output,
                    cache_read,
                    cache_creation,
                },
            );
        }
    }
    out
}

/// Parse a JSON number as `f64`. `None` for missing, null, or non-numeric values
/// — callers map that to "skip row" (input/output) or `0.0` (cache fields).
fn json_f64(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    v.as_f64()
        .or_else(|| v.as_i64().map(|i| i as f64))
        .or_else(|| v.as_u64().map(|u| u as f64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(cost: Value) -> Value {
        json!({
            "anthropic": {
                "models": {
                    "claude-opus-4-8": { "cost": cost }
                }
            }
        })
    }

    #[test]
    fn cache_write_renames_to_cache_creation() {
        let map = ingest_models_dev(&fixture(json!({
            "input": 15.0,
            "output": 75.0,
            "cache_read": 1.5,
            "cache_write": 18.75
        })));
        let price = map.get("anthropic/claude-opus-4-8").expect("opus ingested");
        assert_eq!(price.cache_creation, 18.75);
        // ModelPrice has no cache_write field — the rename is structural.
        assert_eq!(price.input, 15.0);
        assert_eq!(price.output, 75.0);
        assert_eq!(price.cache_read, 1.5);
    }

    #[test]
    fn null_or_absent_cache_becomes_zero_not_input() {
        let null_cache = ingest_models_dev(&fixture(json!({
            "input": 15.0,
            "output": 75.0,
            "cache_read": null
        })));
        let price = null_cache
            .get("anthropic/claude-opus-4-8")
            .expect("opus ingested");
        assert_eq!(price.cache_read, 0.0);
        assert_eq!(price.cache_creation, 0.0);
        assert_ne!(price.cache_read, price.input);

        let absent = ingest_models_dev(&fixture(json!({
            "input": 15.0,
            "output": 75.0
        })));
        let price = absent
            .get("anthropic/claude-opus-4-8")
            .expect("opus ingested");
        assert_eq!(price.cache_read, 0.0);
        assert_eq!(price.cache_creation, 0.0);
    }

    #[test]
    fn zero_input_and_output_row_is_dropped() {
        let map = ingest_models_dev(&fixture(json!({
            "input": 0,
            "output": 0,
            "cache_read": 0,
            "cache_write": 0
        })));
        assert!(
            !map.contains_key("anthropic/claude-opus-4-8"),
            "$0 rows must be dropped"
        );
    }

    #[test]
    fn dated_model_id_key_is_undated_at_ingest() {
        let doc = json!({
            "anthropic": {
                "models": {
                    "claude-haiku-4-5-20251001": {
                        "cost": {
                            "input": 1.0,
                            "output": 5.0,
                            "cache_read": 0.1,
                            "cache_write": 1.25
                        }
                    }
                }
            }
        });
        let map = ingest_models_dev(&doc);
        assert!(map.contains_key("anthropic/claude-haiku-4-5"));
        assert!(!map.contains_key("anthropic/claude-haiku-4-5-20251001"));
    }

    #[test]
    fn non_object_root_and_malformed_entries_yield_empty_or_skip() {
        assert!(ingest_models_dev(&json!([])).is_empty());
        assert!(ingest_models_dev(&json!("nope")).is_empty());
        let partial = ingest_models_dev(&json!({
            "anthropic": {
                "models": {
                    "bad": { "cost": { "input": 1.0 } },
                    "ok": {
                        "cost": { "input": 1.0, "output": 2.0 }
                    }
                }
            }
        }));
        assert_eq!(partial.len(), 1);
        assert!(partial.contains_key("anthropic/ok"));
    }
}
