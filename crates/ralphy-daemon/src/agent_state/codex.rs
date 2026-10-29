//! The Codex map, a copy of the one in `ralphy-agent-codex/src/status.rs`:
//! one status line in, one fold signal out (ADR-0085 D4, D7).

use super::fold::{Signal, StatusLine, WaitKind};

/// Whether a console's Codex reviews its own permission requests. The daemon
/// launches Codex without `--approve-for-me`, so a `PermissionRequest` asks
/// a human (ADR-0085 D4).
const AUTO_REVIEW: bool = false;

// The same map is in `ralphy-agent-codex/src/status.rs`, and it stays two
// copies: the daemon imports no adapter crate (ADR-0059 §5). The shared
// fixture test below holds the two maps equal.
// jscpd:ignore-start
/// The tool Codex asks the operator a question with.
const QUESTION_TOOL: &str = "request_user_input";

/// The Codex map (ADR-0085 D4, D7): one status line in, one fold signal out,
/// or `None` for a line that changes no state (`SessionEnd`, `SubagentStart`,
/// `SubagentStop`, a compaction, an event outside the table).
/// `PermissionRequest` also fires when Codex's auto-reviewer grants the tool
/// and no human is asked, so with `auto_review` it is work, not a wait. Codex
/// sends no event when the operator grants a permission: the wait ends at the
/// tool's `PostToolUse`, when the tool ends.
pub fn codex_signal(line: &StatusLine, auto_review: bool) -> Option<Signal> {
    let tool = line.tool_name.as_deref();
    let signal = match line.event.as_str() {
        "SessionStart" => Signal::SessionBoundary,
        "UserPromptSubmit" => Signal::TurnStart,
        "PreToolUse" if tool == Some(QUESTION_TOOL) => Signal::Wait {
            kind: WaitKind::Question,
            tool_use_id: line.tool_use_id.clone(),
            detail: match line.question.as_deref().map(str::trim) {
                Some(q) if !q.is_empty() => format!("{QUESTION_TOOL}: {q}"),
                _ => QUESTION_TOOL.to_string(),
            },
        },
        "PreToolUse" => Signal::Work,
        "PermissionRequest" if auto_review => Signal::Work,
        "PermissionRequest" => Signal::Wait {
            kind: WaitKind::Permission,
            tool_use_id: line.tool_use_id.clone(),
            detail: match tool {
                Some(t) => format!("permission: {t}"),
                None => "permission".to_string(),
            },
        },
        "PostToolUse" => Signal::WaitEnd {
            tool_use_id: line.tool_use_id.clone(),
        },
        "Stop" => Signal::TurnEnd,
        "Interrupt" => Signal::Interrupt,
        _ => return None,
    };
    Some(signal)
}
// jscpd:ignore-end

/// The map of a console's Codex, which the daemon launches without
/// auto-review.
pub fn launched(line: &StatusLine) -> Option<Signal> {
    codex_signal(line, AUTO_REVIEW)
}

#[cfg(test)]
mod tests {
    use super::super::fold::{Fold, MapFn};
    use super::*;
    use serde_json::Value;

    /// The shared mapping fixture: every row is a sequence of status lines
    /// and the state after each one. The adapter pins ITS fold against
    /// the same file (ADR-0059 §5), so the two cannot drift.
    const MAPPING: &str =
        include_str!("../../../ralphy-agent-codex/tests/fixtures/agent_state_mapping.json");

    // The adapter has the same test against the same fixture (ADR-0059 §5).
    // jscpd:ignore-start
    #[test]
    fn fold_matches_the_codex_mapping_fixture() {
        let rows: Vec<Value> = serde_json::from_str(MAPPING).unwrap();
        assert!(rows.len() >= 15, "the fixture covers the whole table");
        for row in rows {
            let why = row["why"].as_str().unwrap_or_default();
            let map: MapFn = if row["auto_review"] == true {
                |l| codex_signal(l, true)
            } else {
                |l| codex_signal(l, false)
            };
            let mut fold = Fold::new();
            let lines = row["lines"].as_array().expect("lines");
            let states = row["states"].as_array().expect("states");
            assert_eq!(lines.len(), states.len(), "{why}");
            for (i, (line, want)) in lines.iter().zip(states).enumerate() {
                let text = serde_json::to_string(line).unwrap();
                let parsed = StatusLine::parse(&text).expect("a status line");
                let got = fold.apply(&parsed, map).map(|o| {
                    serde_json::json!({
                        "state": o.state.as_str(),
                        "detail": o.detail,
                        "cause": o.cause.map(|c| c.as_str()),
                        "ts": o.ts,
                    })
                });
                let want = (!want.is_null()).then(|| {
                    serde_json::json!({
                        "state": want["state"],
                        "detail": want.get("detail"),
                        "cause": want.get("cause"),
                        "ts": line["ts"],
                    })
                });
                assert_eq!(got, want, "{why}: line {i} {text}");
            }
        }
    }
    // jscpd:ignore-end
}
