//! The Claude map, a copy of the one in `ralphy-agent-claude/src/status.rs`:
//! one status line in, one fold signal out (ADR-0059 §1, ADR-0085 D7).

use super::fold::{Signal, StatusLine, WaitKind};

// The same map is in `ralphy-agent-claude/src/status.rs`, and it stays two
// copies: the daemon imports no adapter crate (ADR-0059 §5). The shared
// fixture test below holds the two maps equal.
// jscpd:ignore-start
/// The Claude map (ADR-0059 §1 as amended by ADR-0085 D7): one status line
/// in, one fold signal out, or `None` for a line that changes no state
/// (`SubagentStop`: a subagent never owns the state; `Notification`, which
/// stays out by ADR-0085 D4; an event outside the table). Claude sends no
/// event on an interrupt (D7.6), so no line maps to `Interrupt`.
pub fn claude_signal(line: &StatusLine) -> Option<Signal> {
    let tool = line.tool_name.as_deref();
    let signal = match line.event.as_str() {
        "SessionStart" => Signal::SessionBoundary,
        "UserPromptSubmit" => Signal::TurnStart,
        "PreToolUse" if tool == Some("AskUserQuestion") => Signal::Wait {
            kind: WaitKind::Question,
            tool_use_id: line.tool_use_id.clone(),
            detail: named("AskUserQuestion", line.question.as_deref()),
        },
        "PreToolUse" => Signal::Work,
        // A tool that returned: the question was answered, the permission
        // granted. A wait ends here, not at the next tool (2026-09-16).
        "PostToolUse" | "ElicitationResult" => Signal::WaitEnd {
            tool_use_id: line.tool_use_id.clone(),
        },
        "PermissionRequest" => Signal::Wait {
            kind: WaitKind::Permission,
            tool_use_id: line.tool_use_id.clone(),
            detail: match tool {
                Some(t) => format!("permission: {t}"),
                None => "permission".to_string(),
            },
        },
        "Elicitation" => Signal::Wait {
            kind: WaitKind::Elicitation,
            tool_use_id: None,
            detail: named("MCP question", line.question.as_deref()),
        },
        "Stop" => Signal::TurnEnd,
        _ => return None,
    };
    Some(signal)
}

/// A wait's detail: `name`, and the question after it when there is one.
fn named(name: &str, question: Option<&str>) -> String {
    match question.map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => format!("{name}: {q}"),
        None => name.to_string(),
    }
}
// jscpd:ignore-end

#[cfg(test)]
mod tests {
    use super::super::fold::Fold;
    use super::*;
    use serde_json::Value;

    /// The shared mapping fixture: every row is a sequence of status lines
    /// and the state after each one. The adapter pins ITS fold against
    /// the same file (ADR-0059 §5), so the two cannot drift.
    const MAPPING: &str =
        include_str!("../../../ralphy-agent-claude/tests/fixtures/agent_state_mapping.json");

    // The adapter has the same test against the same fixture (ADR-0059 §5).
    // jscpd:ignore-start
    #[test]
    fn fold_matches_the_claude_mapping_fixture() {
        let rows: Vec<Value> = serde_json::from_str(MAPPING).unwrap();
        assert!(rows.len() >= 20, "the fixture covers the whole table");
        for row in rows {
            let why = row["why"].as_str().unwrap_or_default();
            let mut fold = Fold::new();
            let lines = row["lines"].as_array().expect("lines");
            let states = row["states"].as_array().expect("states");
            assert_eq!(lines.len(), states.len(), "{why}");
            for (i, (line, want)) in lines.iter().zip(states).enumerate() {
                let text = serde_json::to_string(line).unwrap();
                let parsed = StatusLine::parse(&text).expect("a status line");
                let got = fold.apply(&parsed, claude_signal).map(|o| {
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
