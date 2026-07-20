//! Kimi token-usage capture from `wire.jsonl` session logs (ADR-0008 D7, ADR-0028).

use std::path::{Path, PathBuf};

use ralphy_adapter_support::session_files_appeared;
use ralphy_core::Usage;

/// Parse token usage out of a `kimi-code` 0.28 `wire.jsonl` (ADR-0028 D7).
///
/// The record is top-level (no `message` envelope), dotted-lowercase `type`,
/// camelCase fields:
/// `{"type":"usage.record","usage":{"inputOther":…,"output":…,"inputCacheRead":…,
/// "inputCacheCreation":…},"usageScope":"turn"}`.
///
/// Two traps the shape hides:
/// - Records are per-step INCREMENTS, not a cumulative snapshot, so every matching
///   line is **summed** (the inverse of Codex's keep-last rule).
/// - `context.append_loop_event` lines repeat the same numbers under `event.usage`;
///   folding those double-counts the step. Hence the `usageScope == "turn"` guard
///   on a top-level `usage.record` — nothing else may be counted.
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
        if value.get("type").and_then(|v| v.as_str()) != Some("usage.record") {
            continue;
        }
        if value.get("usageScope").and_then(|v| v.as_str()) != Some("turn") {
            continue;
        }
        let Some(tu) = value.get("usage") else {
            continue;
        };
        let field = |k: &str| tu.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        usage.input += field("inputOther");
        usage.output += field("output");
        usage.cache_read += field("inputCacheRead");
        usage.cache_creation += field("inputCacheCreation");
    }
    usage
}

/// `$RALPHY_KIMI_CODE_DIR/sessions` when set (tests point it at a temp dir), else
/// `$KIMI_CODE_HOME/sessions`, else `<home>/.kimi-code/sessions` (`USERPROFILE` on
/// Windows, `HOME` elsewhere) — the tree 0.28 writes
/// `wd_<repo>_<hash>/session_<uuid>/agents/<agent>/wire.jsonl` session logs into
/// (ADR-0028 D7). Mirrors `ralphy-daemon`'s `kimi_code_dir_path` precedence.
/// `None` when no home is known.
pub(crate) fn kimi_sessions_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_KIMI_CODE_DIR") {
        return Some(PathBuf::from(dir).join("sessions"));
    }
    ralphy_adapter_support::home_scoped_path(
        std::env::var_os("KIMI_CODE_HOME"),
        Path::new(".kimi-code"),
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

/// The vendor session identity of a Kimi run (ADR-0033 §5): the `session_id` on
/// 0.28's own `{"role":"meta","type":"session.resume_hint",…}` stdout line. This
/// is authoritative, unlike the historical positional read of the wire path's
/// parent directory — under 0.28's `…/session_<uuid>/agents/<agent>/wire.jsonl`
/// layout that parent is the agent name (`main`), not a session id (ADR-0028 D7,
/// #239 decision 3). `None` when the stream carries no hint.
pub(crate) fn resume_hint_session_id(stdout: &str) -> Option<String> {
    ralphy_adapter_support::scan_json_lines(stdout, |v| {
        (v.get("type").and_then(|t| t.as_str()) == Some("session.resume_hint"))
            .then(|| v.get("session_id").and_then(|s| s.as_str()))
            .flatten()
            .map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kimi_wire_usage_counts_turn_records_only() {
        // Two per-step `turn` records (summed), plus the two shapes that must be
        // skipped: a `session`-scoped rollup and the `context.append_loop_event`
        // line that repeats the first step's numbers under `event.usage`. Counting
        // either would give 7033/202 instead of 3622/111.
        let jsonl = concat!(
            r#"{"type":"usage.record","model":"kimi-code/k3","usage":{"inputOther":3411,"output":91,"inputCacheRead":10,"inputCacheCreation":1},"usageScope":"turn"}"#,
            "
",
            r#"{"type":"usage.record","model":"kimi-code/k3","usage":{"inputOther":211,"output":20,"inputCacheRead":5,"inputCacheCreation":2},"usageScope":"turn"}"#,
            "
",
            r#"{"type":"usage.record","model":"kimi-code/k3","usage":{"inputOther":3622,"output":111,"inputCacheRead":15,"inputCacheCreation":3},"usageScope":"session"}"#,
            "
",
            r#"{"type":"context.append_loop_event","event":{"usage":{"inputOther":3411,"output":91}}}"#,
            "
",
        );
        let usage = parse_kimi_wire_usage(jsonl, Some("kimi-code/k3".into()));
        assert_eq!(usage.input, 3622, "summed inputOther across the two turns");
        assert_eq!(usage.output, 111);
        assert_eq!(usage.cache_read, 15);
        assert_eq!(usage.cache_creation, 3);
        assert_eq!(usage.model.as_deref(), Some("kimi-code/k3"));
    }

    #[test]
    fn resume_hint_session_id_reads_the_meta_line() {
        let stdout = concat!(
            r#"{"role":"meta","type":"session.start"}"#,
            "
",
            "not json
",
            r#"{"role":"meta","type":"session.resume_hint","session_id":"sess-42"}"#,
            "
",
        );
        assert_eq!(resume_hint_session_id(stdout).as_deref(), Some("sess-42"));
        // A wire PATH is not a session id — the positional read is gone.
        assert_eq!(
            resume_hint_session_id(r#"{"path":"/k/session_x/agents/main/wire.jsonl"}"#),
            None
        );
        assert_eq!(resume_hint_session_id(""), None);
    }

    #[test]
    fn kimi_sessions_dir_honours_ralphy_env_override() {
        let dir = std::env::temp_dir().join(format!("ralphy-kimi-code-{}", std::process::id()));
        std::env::set_var("RALPHY_KIMI_CODE_DIR", &dir);
        let got = kimi_sessions_dir().expect("override always resolves");
        std::env::remove_var("RALPHY_KIMI_CODE_DIR");
        assert_eq!(got, dir.join("sessions"));
    }

    #[test]
    fn parse_kimi_wire_usage_empty_keeps_model() {
        let usage = parse_kimi_wire_usage(
            "not json
{}
",
            Some("kimi-code/k3".into()),
        );
        assert_eq!(usage.total(), 0);
        assert_eq!(usage.model.as_deref(), Some("kimi-code/k3"));
    }
}
