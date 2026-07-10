//! Kimi token-usage capture from `wire.jsonl` session logs (ADR-0008 D7, ADR-0028).

use std::path::{Path, PathBuf};

use ralphy_adapter_support::session_files_appeared;
use ralphy_core::Usage;

/// Parse token usage out of a Kimi `wire.jsonl` (ADR-0008 D7, spike §11.2).
///
/// Kimi writes one `StatusUpdate` per LLM call (per-step), each carrying its OWN
/// `payload.token_usage` — NOT a cumulative snapshot like Codex's
/// `total_token_usage`. So every `StatusUpdate` is **summed** into the running
/// total, the inverse of Codex's keep-last rule.
///
/// Envelope: `{"timestamp":…,"message":{"type":"StatusUpdate","payload":{…}}}`
/// (spike §5); the `message` wrapper is optional so a flat shape still parses.
/// Field mapping: `input_other` → `input`, `output` → `output`,
/// `input_cache_read` → `cache_read`, `input_cache_creation` → `cache_creation`.
fn parse_kimi_wire_usage(jsonl: &str, model: Option<String>) -> Usage {
    let mut usage = Usage {
        model,
        ..Usage::default()
    };
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let obj = value.get("message").unwrap_or(&value);
        if obj.get("type").and_then(|v| v.as_str()) != Some("StatusUpdate") {
            continue;
        }
        let Some(tu) = obj.get("payload").and_then(|p| p.get("token_usage")) else {
            continue;
        };
        let field = |k: &str| tu.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        usage.input += field("input_other");
        usage.output += field("output");
        usage.cache_read += field("input_cache_read");
        usage.cache_creation += field("input_cache_creation");
    }
    usage
}

/// `$KIMI_HOME/sessions` when `KIMI_HOME` is set, else `<home>/.kimi/sessions`
/// (`USERPROFILE` on Windows, `HOME` elsewhere) — the tree Kimi writes
/// `<workdir-hash>/<session-id>/wire.jsonl` session logs into (spike §5).
/// `None` when no home is known.
pub(crate) fn kimi_sessions_dir() -> Option<PathBuf> {
    ralphy_adapter_support::home_scoped_path(
        std::env::var_os("KIMI_HOME"),
        Path::new(".kimi"),
        Path::new("sessions"),
    )
}

/// Snapshot-diff token capture: for each `wire.jsonl` that APPEARED between
/// `before` and `after`, parse its summed usage and fold into one `Usage` (the
/// model carried from the resolved invocation model). Mirrors Codex's
/// `fold_rollout_usage`.
pub(crate) fn fold_wire_usage(
    before: &[PathBuf],
    after: &[PathBuf],
    model: Option<String>,
) -> Usage {
    let parsed: Vec<Usage> = session_files_appeared(before, after)
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse_kimi_wire_usage(&t, model.clone()))
        .collect();
    Usage::fold_usage(&parsed, model.as_deref())
}

/// The vendor session identity of a Kimi run (ADR-0033 §5): the first appeared
/// `wire.jsonl`'s **parent directory name** — the `<session-id>` path segment Kimi
/// nests each session under (`<workdir-hash>/<session-id>/wire.jsonl`, see
/// `kimi_sessions_dir`). `None` when no wire.jsonl appeared.
pub(crate) fn wire_session_id(before: &[PathBuf], after: &[PathBuf]) -> Option<String> {
    session_files_appeared(before, after)
        .first()
        .and_then(|p| p.parent())
        .and_then(|d| d.file_name())
        .and_then(|s| s.to_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kimi_wire_usage_sums_steps_and_maps_fields() {
        // A real wire.jsonl shape: a metadata line, a TurnBegin, TWO StatusUpdate
        // steps (each carrying its OWN per-call token_usage, not a cumulative
        // snapshot), and a TurnEnd. A keep-last implementation would give
        // 4776/37/9472/0 and fail this assertion.
        let jsonl = concat!(
            r#"{"timestamp":0,"message":{"type":"metadata","payload":{}}}"#,
            "\n",
            r#"{"timestamp":1,"message":{"type":"TurnBegin","payload":{}}}"#,
            "\n",
            r#"{"timestamp":2,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":4776,"output":37,"input_cache_read":9472,"input_cache_creation":0}}}}"#,
            "\n",
            r#"{"timestamp":3,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":200,"input_cache_read":50,"input_cache_creation":25}}}}"#,
            "\n",
            r#"{"timestamp":4,"message":{"type":"TurnEnd","payload":{}}}"#,
            "\n",
        );
        let usage = parse_kimi_wire_usage(jsonl, Some("kimi-code/kimi-for-coding".into()));
        assert_eq!(usage.input, 4876, "summed input_other across both steps");
        assert_eq!(usage.output, 237);
        assert_eq!(usage.cache_read, 9522);
        assert_eq!(usage.cache_creation, 25);
        assert_eq!(usage.model.as_deref(), Some("kimi-code/kimi-for-coding"));
        assert_eq!(usage.total(), 14660);
    }

    #[test]
    fn wire_session_id_takes_parent_dir_name() {
        let after = vec![PathBuf::from("/k/hash7/sess-42/wire.jsonl")];
        assert_eq!(wire_session_id(&[], &after).as_deref(), Some("sess-42"));
        // Nothing appeared → None.
        assert_eq!(wire_session_id(&[], &[]), None);
    }

    #[test]
    fn parse_kimi_wire_usage_empty_keeps_model() {
        let usage =
            parse_kimi_wire_usage("not json\n{}\n", Some("kimi-code/kimi-for-coding".into()));
        assert_eq!(usage.total(), 0);
        assert_eq!(usage.model.as_deref(), Some("kimi-code/kimi-for-coding"));
    }
}
