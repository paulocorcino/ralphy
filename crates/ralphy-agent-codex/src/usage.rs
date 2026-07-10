//! Codex token-usage capture from rollout JSONL session logs (ADR-0008 D5).

use std::path::{Path, PathBuf};

use ralphy_adapter_support::session_files_appeared;
use ralphy_core::Usage;

/// Parse the cumulative token usage out of a Codex rollout JSONL (ADR-0008 D5).
///
/// Codex writes `event_msg` lines whose `payload.type == "token_count"` carry a
/// `payload.info.total_token_usage` snapshot that is **cumulative** for the
/// session; many `info` objects are `{}` or `null`, so the parser keeps the LAST
/// populated `total_token_usage` (the final cumulative figure for the phase).
///
/// Mapping (the load-bearing trap): Codex's `input_tokens` **includes** the
/// cached subset, so `input = input_tokens − cached_input_tokens` and
/// `cache_read = cached_input_tokens` — summing raw would double-count.
/// `cache_creation` is `0` (Codex reports no cache-write split) and
/// `reasoning_output_tokens` is NOT added: Codex's own `total_tokens` reconciles
/// as `input_tokens + output_tokens`, so reasoning already sits inside `output`.
/// `model` is the resolved invocation model (the rollout has no per-event model).
fn parse_codex_rollout_usage(jsonl: &str, model: Option<String>) -> Usage {
    let mut last: Option<serde_json::Value> = None;
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // opencode-style envelope: the real fields live under `payload`; fall
        // back to the value itself so a flat shape still parses.
        let payload = value.get("payload").unwrap_or(&value);
        if payload.get("type").and_then(|v| v.as_str()) != Some("token_count") {
            continue;
        }
        let Some(ttu) = payload
            .get("info")
            .and_then(|info| info.get("total_token_usage"))
        else {
            continue;
        };
        // Keep only populated snapshots; `{}`/null `info` must not clobber the
        // last good cumulative total.
        if ttu.is_object() {
            last = Some(ttu.clone());
        }
    }

    let mut usage = Usage {
        model,
        ..Usage::default()
    };
    if let Some(ttu) = last {
        let field = |k: &str| ttu.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let cached = field("cached_input_tokens");
        usage.input = field("input_tokens").saturating_sub(cached);
        usage.output = field("output_tokens");
        usage.cache_read = cached;
        usage.cache_creation = 0;
    }
    usage
}

/// `$CODEX_HOME/sessions` when `CODEX_HOME` is set (matching Codex's own
/// resolution), else `<home>/.codex/sessions` (`USERPROFILE` on Windows, `HOME`
/// elsewhere) — the tree Codex writes `rollout-*.jsonl` session logs into.
/// `None` when no home is known. Mirrors the home logic in `codex_config_path`.
pub(crate) fn codex_sessions_dir() -> Option<PathBuf> {
    ralphy_adapter_support::home_scoped_path(
        std::env::var_os("CODEX_HOME"),
        Path::new(".codex"),
        Path::new("sessions"),
    )
}

/// Snapshot-diff token capture: for each rollout that APPEARED between `before`
/// and `after`, parse its cumulative usage and fold into one `Usage` (the model
/// carried from the resolved invocation model). Mirrors `ClaudeAgent::execute`'s
/// before/after/appeared/fold.
pub(crate) fn fold_rollout_usage(
    before: &[PathBuf],
    after: &[PathBuf],
    model: Option<String>,
) -> Usage {
    let parsed: Vec<Usage> = session_files_appeared(before, after)
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse_codex_rollout_usage(&t, model.clone()))
        .collect();
    // Every parsed record carries the same invocation `model`, so heaviest-with-
    // model and the fallback both resolve to it — including the zero-appeared case
    // (empty items → fallback model, zero tokens), matching the old seed result.
    Usage::fold_usage(&parsed, model.as_deref())
}

/// The vendor session identity of a Codex run (ADR-0033 §5): the first appeared
/// `rollout-*.jsonl` file's **stem** (`rollout-<ts>-<uuid>`). This is the dedup
/// contract the future usage-scan MUST key Codex sessions on identically. `None`
/// when no rollout appeared.
pub(crate) fn rollout_session_id(before: &[PathBuf], after: &[PathBuf]) -> Option<String> {
    session_files_appeared(before, after)
        .first()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_codex_rollout_usage ───────────────────────────────────────────

    #[test]
    fn parse_codex_rollout_usage_maps_cached_subtract_and_keeps_last() {
        // A real rollout shape: an empty `info`, then a populated cumulative
        // `total_token_usage`, then a trailing null `info` that must NOT clobber
        // the last good snapshot. `input_tokens` includes the cached subset, so
        // the mapped `input` is `841957 - 735616`; reasoning is already inside
        // `output` so it is not added.
        let jsonl = concat!(
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{}}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":841957,"cached_input_tokens":735616,"output_tokens":10466,"reasoning_output_tokens":4242,"total_tokens":852423},"last_token_usage":{"input_tokens":10}}}}"#,
            "\n",
            r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#,
            "\n",
        );
        let usage = parse_codex_rollout_usage(jsonl, Some("gpt-5-codex".into()));
        assert_eq!(usage.input, 106341, "input = input_tokens - cached");
        assert_eq!(usage.output, 10466, "output excludes reasoning");
        assert_eq!(usage.cache_read, 735616);
        assert_eq!(usage.cache_creation, 0);
        assert_eq!(usage.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(usage.total(), 852423, "reconciles with Codex's own total");
    }

    #[test]
    fn rollout_session_id_takes_first_appeared_stem() {
        let after = vec![PathBuf::from(
            "/s/rollout-2026-01-01T00-00-00-uuid.jsonl",
        )];
        assert_eq!(
            rollout_session_id(&[], &after).as_deref(),
            Some("rollout-2026-01-01T00-00-00-uuid")
        );
        // Nothing appeared → None.
        assert_eq!(rollout_session_id(&[], &[]), None);
    }

    #[test]
    fn parse_codex_rollout_usage_empty_keeps_model() {
        // No populated event → zeroed counts, but the model is preserved.
        let usage = parse_codex_rollout_usage("not json\n{}\n", Some("gpt-5-codex".into()));
        assert_eq!(usage.total(), 0);
        assert_eq!(usage.model.as_deref(), Some("gpt-5-codex"));
    }
}
