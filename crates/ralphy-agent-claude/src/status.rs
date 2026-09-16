//! The agent's own state from Claude Code's hooks (docs/adr/0059 §1, §3).
//!
//! `ralphy hook status` appends one JSON line per hook event to
//! `$RALPHY_STATUS_FILE`; this module is the other end: [`fold_line`] maps a
//! line to a state per the §1 table, and [`Watcher`] tails the file from a
//! thread while a child runs — plan, interactive and headless alike — and
//! calls `emit::agent_state` on a CHANGE only, so a fold downstream is a
//! transition. The mapping is pinned by `tests/fixtures/agent_state_mapping.json`,
//! which the daemon's own copy of the fold is pinned by too (§5: the daemon
//! never imports this crate).
//!
//! Nothing here reads the terminal: a state that is guessed is worse than one
//! that is absent (§7).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::Value;

/// The file name under the run dir, beside `status.flag`.
pub(crate) const STATUS_FILE: &str = "agent-status.jsonl";

/// The environment variable the hook reads the path from.
pub(crate) const STATUS_ENV: &str = "RALPHY_STATUS_FILE";

/// One observed state, folded from one hook line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observed {
    pub state: &'static str,
    pub detail: Option<String>,
    pub interrupted: bool,
    pub ts: String,
}

/// The §1 table, one hook line in, one state out — or `None` for a line that
/// changes no state (`SubagentStop`: a subagent never owns the state; an
/// event outside the table; a line that is not JSON).
pub(crate) fn fold_line(line: &str) -> Option<Observed> {
    let v: Value = serde_json::from_str(line).ok()?;
    let event = v.get("event").and_then(Value::as_str).unwrap_or("");
    let tool = v.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let ts = v
        .get("ts")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let interrupted = v
        .get("interrupted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let (state, detail) = match event {
        "SessionStart" => ("done", None),
        "UserPromptSubmit" => ("working", None),
        "PreToolUse" if tool == "AskUserQuestion" => {
            ("waiting", Some(question_detail(v.get("tool_input"))))
        }
        "PreToolUse" => ("working", None),
        "PermissionRequest" => (
            "waiting",
            Some(if tool.is_empty() {
                "permission".to_string()
            } else {
                format!("permission: {tool}")
            }),
        ),
        "Stop" => ("done", None),
        _ => return None,
    };
    Some(Observed {
        state,
        detail,
        interrupted: state == "done" && interrupted,
        ts,
    })
}

/// What an `AskUserQuestion` asks, from its `tool_input`: the first question's
/// text when the shape is the vendor's (`questions: [{ question }]`), else the
/// tool name alone.
fn question_detail(input: Option<&Value>) -> String {
    let text = input
        .and_then(|i| i.get("questions"))
        .and_then(Value::as_array)
        .and_then(|qs| qs.first())
        .and_then(|q| q.get("question"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match text {
        Some(t) => format!("AskUserQuestion: {t}"),
        None => "AskUserQuestion".to_string(),
    }
}

/// The tail: reads what was appended since the last poll, folds each
/// complete line, and reports only the transitions.
pub(crate) struct Tail {
    path: PathBuf,
    offset: u64,
    carry: Vec<u8>,
    last: Option<&'static str>,
}

impl Tail {
    pub(crate) fn new(path: PathBuf) -> Self {
        Tail {
            path,
            offset: 0,
            carry: Vec::new(),
            last: None,
        }
    }

    /// The states that CHANGED since the last poll, in order. A `waiting`
    /// with a new detail counts as a change (a second question is news); the
    /// same state with the same detail does not.
    pub(crate) fn poll(&mut self) -> Vec<Observed> {
        let mut out = Vec::new();
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return out;
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return out;
        }
        let mut buf = Vec::new();
        let Ok(n) = file.read_to_end(&mut buf) else {
            return out;
        };
        self.offset += n as u64;
        self.carry.extend_from_slice(&buf);
        while let Some(nl) = self.carry.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.carry.drain(..=nl).collect();
            let Ok(text) = std::str::from_utf8(&line) else {
                continue;
            };
            if let Some(obs) = fold_line(text.trim()) {
                if self.last != Some(obs.state) || obs.state == "waiting" {
                    self.last = Some(obs.state);
                    out.push(obs);
                }
            }
        }
        out
    }
}

/// Tails the status file from a thread for as long as a child runs and emits
/// each transition. `start` truncates the file (a previous issue's lines are
/// not this child's) and returns the path to hand the child as
/// `RALPHY_STATUS_FILE`; `stop` takes the last look and joins.
pub(crate) struct Watcher {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    path: PathBuf,
}

impl Watcher {
    pub(crate) fn start(run_dir: &Path) -> Self {
        let path = run_dir.join(STATUS_FILE);
        // Truncate, never fail: a status file that cannot be reset is a
        // status that may be stale, not a run that may not start.
        if let Err(e) = std::fs::write(&path, b"") {
            tracing::warn!(error = %e, path = %path.display(), "could not reset the agent-status file");
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let mut tail = Tail::new(path.clone());
        let handle = std::thread::spawn(move || loop {
            for obs in tail.poll() {
                emit(&obs);
            }
            if flag.load(Ordering::SeqCst) {
                // One last look: a `Stop` line lands right before the child
                // exits, and the loop must not lose it to the join.
                for obs in tail.poll() {
                    emit(&obs);
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        });
        Watcher {
            stop,
            handle: Some(handle),
            path,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            // A panicked watcher thread has nothing to report; the run goes on.
            let _joined = h.join();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _joined = h.join();
        }
    }
}

fn emit(obs: &Observed) {
    ralphy_core::emit::agent_state(obs.state, &obs.ts, obs.detail.as_deref(), obs.interrupted);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared mapping fixture: every row is one hook line and the state
    /// it must fold to. The daemon pins ITS fold against the same file
    /// (ADR-0059 §5), so the two cannot drift.
    const MAPPING: &str = include_str!("../tests/fixtures/agent_state_mapping.json");

    #[test]
    fn fold_line_matches_the_shared_mapping_fixture() {
        let rows: Vec<Value> = serde_json::from_str(MAPPING).unwrap();
        assert!(rows.len() >= 8, "the fixture covers the whole table");
        for row in rows {
            let line = serde_json::to_string(&row["line"]).unwrap();
            let got = fold_line(&line);
            match row["state"].as_str() {
                None => assert_eq!(got, None, "{line} must fold to nothing"),
                Some(state) => {
                    let got = got.unwrap_or_else(|| panic!("{line} must fold to {state}"));
                    assert_eq!(got.state, state, "{line}");
                    assert_eq!(
                        got.detail.as_deref(),
                        row["detail"].as_str(),
                        "detail for {line}"
                    );
                    assert_eq!(
                        got.interrupted,
                        row["interrupted"].as_bool().unwrap_or(false),
                        "interrupted for {line}"
                    );
                }
            }
        }
    }

    /// The tail reports transitions only, survives a partial line across two
    /// polls, and treats a second `waiting` with a new question as news.
    #[test]
    fn tail_reports_transitions_and_handles_partial_lines() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATUS_FILE);
        let mut f = std::fs::File::create(&path).unwrap();
        let mut tail = Tail::new(path.clone());
        assert!(tail.poll().is_empty());

        let line = |event: &str, tool: &str| {
            format!(
                r#"{{"event":"{event}","tool_name":{},"tool_input":null,"interrupted":false,"ts":"t"}}"#,
                if tool.is_empty() {
                    "null".to_string()
                } else {
                    format!("\"{tool}\"")
                }
            )
        };
        writeln!(f, "{}", line("UserPromptSubmit", "")).unwrap();
        writeln!(f, "{}", line("PreToolUse", "Bash")).unwrap();
        writeln!(f, "{}", line("PreToolUse", "Edit")).unwrap();
        f.flush().unwrap();
        let got: Vec<&str> = tail.poll().iter().map(|o| o.state).collect::<Vec<_>>();
        assert_eq!(got, vec!["working"], "three working lines, one transition");

        // A partial line: nothing until the newline lands.
        let stop = line("Stop", "");
        let (head, rest) = stop.split_at(10);
        write!(f, "{head}").unwrap();
        f.flush().unwrap();
        assert!(tail.poll().is_empty());
        writeln!(f, "{rest}").unwrap();
        f.flush().unwrap();
        let got: Vec<&str> = tail.poll().iter().map(|o| o.state).collect();
        assert_eq!(got, vec!["done"]);

        // Two questions in a row are two `waiting`s.
        let ask = |q: &str| {
            format!(
                r#"{{"event":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{{"questions":[{{"question":"{q}"}}]}},"interrupted":false,"ts":"t"}}"#
            )
        };
        writeln!(f, "{}", ask("port?")).unwrap();
        writeln!(f, "{}", ask("host?")).unwrap();
        f.flush().unwrap();
        let got: Vec<String> = tail
            .poll()
            .into_iter()
            .map(|o| o.detail.unwrap_or_default())
            .collect();
        assert_eq!(
            got,
            vec!["AskUserQuestion: port?", "AskUserQuestion: host?"]
        );
    }

    /// A `Stop` the vendor marks as an interrupt is `done{interrupted}`;
    /// the flag is ignored on any other state.
    #[test]
    fn interrupted_rides_only_a_done() {
        let done = fold_line(r#"{"event":"Stop","interrupted":true,"ts":"t"}"#).unwrap();
        assert_eq!((done.state, done.interrupted), ("done", true));
        let working =
            fold_line(r#"{"event":"UserPromptSubmit","interrupted":true,"ts":"t"}"#).unwrap();
        assert_eq!((working.state, working.interrupted), ("working", false));
    }
}
