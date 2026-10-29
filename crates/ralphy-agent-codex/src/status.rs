//! The agent's own state from Codex's hooks (ADR-0085 D2, D4, D7).
//!
//! The run gives Codex its status hooks inline (`-c hooks.<Event>=...`), and
//! each hook runs `ralphy hook status codex`, which appends one status line
//! per event to `$RALPHY_STATUS_FILE`. This module is the other end:
//! [`codex_signal`] maps a line to a fold signal, the shared fold applies the
//! D7 rules, and [`Watcher`] tails the file from a thread while a child runs.
//! The map and the fold are pinned by `tests/fixtures/agent_state_mapping.json`,
//! which the daemon's own copy is pinned by too (ADR-0059 §5).

use std::path::Path;

use ralphy_adapter_support::agent_state::{self, Signal, StatusLine, WaitKind};

/// The file name under the run dir.
pub(crate) const STATUS_FILE: &str = "agent-status.jsonl";

/// The environment variable the hook reads the path from.
pub(crate) const STATUS_ENV: &str = "RALPHY_STATUS_FILE";

/// Whether a run's Codex reviews its own permission requests. Ralphy never
/// passes `--approve-for-me`, so a `PermissionRequest` asks a human (D4).
const AUTO_REVIEW: bool = false;

// The same map is in `ralphy-daemon/src/agent_state/codex.rs`, and it stays
// two copies: the daemon imports no adapter crate (ADR-0059 §5). The shared
// fixture test holds the two maps equal.
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
pub(crate) fn codex_signal(line: &StatusLine, auto_review: bool) -> Option<Signal> {
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

/// The map of a run's Codex, which Ralphy launches without auto-review.
fn launched(line: &StatusLine) -> Option<Signal> {
    codex_signal(line, AUTO_REVIEW)
}

/// The run dir's status file, tailed from a thread while a child runs; each
/// state change is emitted as an `agent state` event. `start` truncates the
/// file and returns the path to hand the child as `RALPHY_STATUS_FILE`.
pub(crate) struct Watcher(agent_state::Watcher);

impl Watcher {
    pub(crate) fn start(run_dir: &Path) -> anyhow::Result<Self> {
        agent_state::Watcher::start(run_dir.join(STATUS_FILE), launched).map(Watcher)
    }

    pub(crate) fn path(&self) -> &Path {
        self.0.path()
    }

    pub(crate) fn stop(self) {
        self.0.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ralphy_adapter_support::agent_state::Fold;
    use serde_json::Value;

    /// The shared mapping fixture: every row is a sequence of status lines
    /// and the state after each one. The daemon pins ITS fold against the
    /// same file (ADR-0059 §5), so the two cannot drift.
    const MAPPING: &str = include_str!("../tests/fixtures/agent_state_mapping.json");

    // The daemon has the same test against the same fixture (ADR-0059 §5).
    // jscpd:ignore-start
    #[test]
    fn fold_matches_the_shared_mapping_fixture() {
        let rows: Vec<Value> = serde_json::from_str(MAPPING).unwrap();
        assert!(rows.len() >= 15, "the fixture covers the whole table");
        for row in rows {
            let why = row["why"].as_str().unwrap_or_default();
            let map: agent_state::MapFn = if row["auto_review"] == true {
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

    // ---- a run, end to end through `tracing` (ADR-0059 §3) -------------
    //
    // The watcher emits from its own thread, so the capture is a
    // process-global subscriber: nextest runs each test in a process of its
    // own, and this is the only test of the crate that installs one.
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::Layer;

    type Sink = Arc<Mutex<Vec<(String, String)>>>;

    /// Keeps the `state` and `detail` of each `agent state` event.
    struct Capture(Sink);

    impl<S: tracing::Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            #[derive(Default)]
            struct Fields {
                message: String,
                state: String,
                detail: String,
            }
            impl tracing::field::Visit for Fields {
                fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
                    match f.name() {
                        "state" => self.state = v.into(),
                        "detail" => self.detail = v.into(),
                        _ => {}
                    }
                }
                fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                    if f.name() == "message" {
                        self.message = format!("{v:?}");
                    }
                }
            }
            let mut fields = Fields::default();
            event.record(&mut fields);
            if fields.message == ralphy_core::emit::AGENT_STATE_MSG {
                self.0
                    .lock()
                    .expect("sink")
                    .push((fields.state, fields.detail));
            }
        }
    }

    /// A run whose child (the run hook, standing in for `codex exec` and its
    /// hooks) appends a `PermissionRequest` line to the run's status file
    /// emits one `agent state` event: `waiting`, with the tool.
    #[test]
    fn a_codex_run_emits_the_agent_state_of_its_hooks() {
        let sink: Sink = Arc::new(Mutex::new(Vec::new()));
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(Capture(sink.clone())),
        )
        .expect("this test installs the one global subscriber of its process");
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let ws = ralphy_core::Workspace::new(&repo);
        let run_dir = temp.path().join("run");
        let status_file = run_dir.join(STATUS_FILE);
        let agent = crate::CodexAgent::new(Some("gpt-5-codex".into()), run_dir).with_run_hook(
            move || {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&status_file)?;
                writeln!(
                    f,
                    r#"{{"event":"PermissionRequest","session_id":"s1","tool_name":"Bash","ts":"t1"}}"#
                )?;
                Ok(ralphy_adapter_support::HeadlessRun {
                    stdout: String::new(),
                    log: String::new(),
                    exited_cleanly: true,
                    timed_out: false,
                    idle_killed: false,
                    stopped: false,
                    exit_code: Some(0),
                })
            },
        );
        let plan = ralphy_core::Plan {
            path: ws.plan_path(),
            open_steps: 1,
            recommended_model: Some("high".into()),
            usage: ralphy_core::Usage::default(),
            session_id: None,
        };
        ralphy_core::Agent::execute(&agent, &plan, &ws).expect("the execute ends");
        let got = sink.lock().expect("sink").clone();
        assert_eq!(
            got,
            [("waiting".to_string(), "permission: Bash".to_string())]
        );
    }
}
