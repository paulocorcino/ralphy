//! The OpenCode map, a copy of the one in `ralphy-agent-opencode/src/status.rs`:
//! one line of the state plugin in, one fold signal out (ADR-0085 D2, D7).
//! Also the plugin file a console loads, and the config entry that names it.

use std::path::Path;

use super::fold::{Signal, StatusLine, WaitKind};

/// The plugin source, the same file the adapter writes for a run.
pub const STATE_PLUGIN: &str = include_str!("../../../../assets/opencode/ralphy-state-plugin.js");

/// The plugin's first line: it changes no state, but it is the proof that
/// the plugin runs, so it ends a console's silence (ADR-0085 D9).
pub const PLUGIN_LOADED: &str = "plugin.loaded";

// The same map is in `ralphy-agent-opencode/src/status.rs`, and it stays two
// copies: the daemon imports no adapter crate (ADR-0059 §5). The shared
// fixture test below holds the two maps equal.
// jscpd:ignore-start
/// The tool OpenCode asks the operator a question with.
const QUESTION_TOOL: &str = "question";

/// The OpenCode map (ADR-0085 D7): one line of the state plugin in, one fold
/// signal out, or `None` for a line that changes no state. The plugin's
/// first line, `plugin.loaded`, is only the proof that it runs. A
/// `session.created` line is a new lead session: the plugin writes none for
/// a subagent's session, and it names a subagent's other lines with an
/// `agent_id`. A reply carries the id of the request it answers, so it ends
/// only that request's wait.
pub fn opencode_signal(line: &StatusLine) -> Option<Signal> {
    let request = line.tool_use_id.clone();
    let signal = match line.event.as_str() {
        "session.created" => Signal::SessionBoundary,
        "session.status" => Signal::Work,
        "permission.asked" => Signal::Wait {
            kind: WaitKind::Permission,
            tool_use_id: request,
            detail: match line.tool_name.as_deref() {
                Some(t) => format!("permission: {t}"),
                None => "permission".to_string(),
            },
        },
        "question.asked" => Signal::Wait {
            kind: WaitKind::Question,
            tool_use_id: request,
            detail: match line.question.as_deref().map(str::trim) {
                Some(q) if !q.is_empty() => format!("{QUESTION_TOOL}: {q}"),
                _ => QUESTION_TOOL.to_string(),
            },
        },
        "permission.replied" | "question.replied" | "question.rejected" => Signal::WaitEnd {
            tool_use_id: request,
        },
        "session.idle" => Signal::TurnEnd,
        _ => return None,
    };
    Some(signal)
}
// jscpd:ignore-end

// The adapter names a run's plugin the same way, in
// `ralphy-agent-opencode/src/command.rs` (ADR-0059 §5).
/// The `plugin` entry that names `path`: a `file:///` URL with forward
/// slashes, the form OpenCode loaded on Windows and on Linux (measured with
/// OpenCode 1.18.4 and 1.19.0). A Windows verbatim prefix is dropped.
pub fn plugin_entry(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    format!("file:///{}", text.trim_start_matches('/'))
}

/// The `OPENCODE_CONFIG_CONTENT` of a console: the state plugin and nothing
/// else. OpenCode merges it with the operator's own config, so their
/// settings and plugins still load.
pub fn console_config(plugin: &Path) -> String {
    serde_json::json!({ "plugin": [plugin_entry(plugin)] }).to_string()
}

#[cfg(test)]
mod tests {
    use super::super::fold::Fold;
    use super::*;
    use serde_json::Value;

    /// The shared mapping fixture: every row is a sequence of status lines
    /// and the state after each one. The adapter pins ITS fold against
    /// the same file (ADR-0059 §5), so the two cannot drift.
    const MAPPING: &str =
        include_str!("../../../ralphy-agent-opencode/tests/fixtures/agent_state_mapping.json");

    // The adapter has the same test against the same fixture (ADR-0059 §5).
    // jscpd:ignore-start
    #[test]
    fn fold_matches_the_opencode_mapping_fixture() {
        let rows: Vec<Value> = serde_json::from_str(MAPPING).unwrap();
        assert!(rows.len() >= 10, "the fixture covers the whole table");
        for row in rows {
            let why = row["why"].as_str().unwrap_or_default();
            let mut fold = Fold::new();
            let lines = row["lines"].as_array().expect("lines");
            let states = row["states"].as_array().expect("states");
            assert_eq!(lines.len(), states.len(), "{why}");
            for (i, (line, want)) in lines.iter().zip(states).enumerate() {
                let text = serde_json::to_string(line).unwrap();
                let parsed = StatusLine::parse(&text).expect("a status line");
                let got = fold.apply(&parsed, opencode_signal).map(|o| {
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
