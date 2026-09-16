//! A console's agent state from the vendor's hooks (docs/adr/0059 §5).
//!
//! The daemon writes a per-session settings file registering the §4 hook set
//! (each `<ralphy exe> hook status`), points the child at a per-session
//! status file through `RALPHY_STATUS_FILE`, and tails that file from the
//! session's own pump tick. The fold from hook line to state is the §1 table
//! **duplicated** here — the daemon never imports the adapter (ADR-0032 §10)
//! — and pinned against the adapter's fixture
//! (`crates/ralphy-agent-claude/tests/fixtures/agent_state_mapping.json`) so
//! the two cannot drift. Staleness (§6) is applied at READ time: a `working`
//! older than the interactive idle window renders `unknown`; a `waiting`
//! never goes stale; the state dies with the PTY and is never persisted.
//!
//! Nothing here reads the terminal (§7).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

/// The idle window a `working` may age past before it renders `unknown` —
/// the interactive default `ralphy-core` reaps a wedged run at
/// (`DEFAULT_INTERACTIVE_IDLE_MINUTES`), restated here because the daemon
/// does not import core. A console has no reaper; this is only how long a
/// green dot is believed.
pub const STALE_AFTER: Duration = Duration::from_secs(45 * 60);

/// The environment variable the hook reads the status path from.
pub const STATUS_ENV: &str = "RALPHY_STATUS_FILE";

/// One observed state, folded from one hook line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub state: &'static str,
    pub detail: Option<String>,
    /// The hook line's own `ts`, verbatim (RFC 3339 from the hook's clock).
    pub since: String,
    /// When THIS daemon read the line — the staleness clock, independent of
    /// the hook's string timestamp.
    pub seen: SystemTime,
}

/// The wire shape on `/api/sessions` and the fleet: `state` is one of
/// `working`, `waiting`, `done`, `blocked`, `unknown`; `since` the hook's
/// timestamp; `detail` what a `waiting` agent asks.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentState {
    pub state: String,
    pub since: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The §1 table, one hook line in, one state out — or `None` for a line that
/// changes no state. Byte-for-byte the adapter's fold; the fixture test is
/// the proof.
pub fn fold_line(line: &str) -> Option<Observed> {
    let v: Value = serde_json::from_str(line).ok()?;
    let event = v.get("event").and_then(Value::as_str).unwrap_or("");
    let tool = v.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let since = v
        .get("ts")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let (state, detail) = match event {
        "SessionStart" => ("done", None),
        "UserPromptSubmit" => ("working", None),
        "PreToolUse" if tool == "AskUserQuestion" => {
            ("waiting", Some(question_detail(v.get("tool_input"))))
        }
        "PreToolUse" => ("working", None),
        // A tool that returned: the question was answered, the permission
        // granted — `waiting` ends here, not at the next tool (2026-09-16).
        "PostToolUse" => ("working", None),
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
        since,
        seen: SystemTime::now(),
    })
}

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

/// What a reader is told about an observation `now` (§6): `working` past
/// [`STALE_AFTER`] is `unknown` — the hook that would have said otherwise
/// never came, and a green dot on a wedged child is the lie this rule
/// exists to prevent; every other state is what it was.
pub fn render(obs: &Observed, now: SystemTime) -> AgentState {
    let stale = obs.state == "working"
        && now
            .duration_since(obs.seen)
            .is_ok_and(|age| age > STALE_AFTER);
    AgentState {
        state: if stale { "unknown" } else { obs.state }.to_string(),
        since: obs.since.clone(),
        detail: obs.detail.clone(),
    }
}

/// What one poll of the tail saw: the state TRANSITIONS, in order, and
/// whether ANY line folded — a repeated `working` is not a transition but it
/// is proof of life, and the staleness clock (§6) must read it.
#[derive(Debug, Default)]
pub struct Polled {
    pub transitions: Vec<Observed>,
    pub activity: bool,
}

/// The tail: reads what was appended since the last poll, folds each
/// complete line, and reports the transitions — a change of state, or a
/// `waiting` with a new detail (a second question is news; the same
/// question re-asked is not).
pub struct Tail {
    path: PathBuf,
    offset: u64,
    carry: Vec<u8>,
    last: Option<(&'static str, Option<String>)>,
}

impl Tail {
    pub fn new(path: PathBuf) -> Self {
        Tail {
            path,
            offset: 0,
            carry: Vec::new(),
            last: None,
        }
    }

    pub fn poll(&mut self) -> Polled {
        let mut out = Polled::default();
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
                out.activity = true;
                let key = (obs.state, obs.detail.clone());
                if self.last.as_ref() != Some(&key) {
                    self.last = Some(key);
                    out.transitions.push(obs);
                }
            }
        }
        out
    }
}

/// The two files one console's hooks need, under `<store>/sessions/`: the
/// settings file the vendor is launched with and the status file the hooks
/// append to. Both are the session's and go with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusFiles {
    pub settings: PathBuf,
    pub status: PathBuf,
}

impl StatusFiles {
    pub fn for_session(sessions_dir: &Path, id: u64) -> Self {
        StatusFiles {
            settings: sessions_dir.join(format!("{id}.settings.json")),
            status: sessions_dir.join(format!("{id}.agent-status.jsonl")),
        }
    }

    /// Write the settings file (the §4 hook set, each `<exe> hook status`)
    /// and an empty status file. The settings are the hooks and NOTHING
    /// else: the vendor merges `--settings` over the operator's own file, so
    /// their permissions and hooks stay (ADR-0059 §5).
    pub fn write(&self, exe: &Path) -> std::io::Result<()> {
        if let Some(dir) = self.settings.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.settings, console_settings_json(exe))?;
        std::fs::write(&self.status, b"")
    }

    pub fn remove(&self) {
        // Best effort, and quiet: a session that ended has nothing to report
        // about a file the daemon itself created.
        let _settings = std::fs::remove_file(&self.settings);
        let _status = std::fs::remove_file(&self.status);
    }
}

/// The console's settings document: the §4 hook set on every event, each
/// running `"<exe>" hook status`. The same seven events the adapter registers
/// (`SessionStart`, `UserPromptSubmit`, `PreToolUse *`, `PostToolUse *`,
/// `PermissionRequest *`, `Stop`, `SubagentStop`), and none of the ones it
/// rejected.
pub fn console_settings_json(exe: &Path) -> String {
    let command = format!("\"{}\" hook status", exe.display());
    let entry = |matcher: &str| {
        serde_json::json!([{
            "matcher": matcher,
            "hooks": [ { "type": "command", "command": command } ]
        }])
    };
    let settings = serde_json::json!({
        "hooks": {
            "SessionStart": entry(""),
            "UserPromptSubmit": entry(""),
            "PreToolUse": entry("*"),
            "PostToolUse": entry("*"),
            "PermissionRequest": entry("*"),
            "Stop": entry(""),
            "SubagentStop": entry(""),
        }
    });
    serde_json::to_string_pretty(&settings).expect("settings serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The adapter's fixture, by path: the ONE file both folds are pinned by
    /// (ADR-0059 §5). Moving or editing it reds both crates.
    const MAPPING: &str =
        include_str!("../../ralphy-agent-claude/tests/fixtures/agent_state_mapping.json");

    #[test]
    fn fold_line_matches_the_adapters_mapping_fixture() {
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
                    assert_eq!(got.detail.as_deref(), row["detail"].as_str(), "{line}");
                }
            }
        }
    }

    /// §6: only a `working` goes stale, and only past the window.
    #[test]
    fn render_ages_working_into_unknown_and_nothing_else() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let obs = |state: &'static str| Observed {
            state,
            detail: None,
            since: "t".into(),
            seen: t0,
        };
        let fresh = t0 + STALE_AFTER - Duration::from_secs(1);
        let old = t0 + STALE_AFTER + Duration::from_secs(1);
        assert_eq!(render(&obs("working"), fresh).state, "working");
        assert_eq!(render(&obs("working"), old).state, "unknown");
        assert_eq!(render(&obs("waiting"), old).state, "waiting");
        assert_eq!(render(&obs("done"), old).state, "done");
        // A clock that went backwards is not an age.
        assert_eq!(
            render(&obs("working"), t0 - Duration::from_secs(5)).state,
            "working"
        );
    }

    /// Transitions only — but every folded line is ACTIVITY (§6's clock), and
    /// a `waiting` repeats only with a new detail.
    #[test]
    fn tail_reports_transitions_and_activity_separately() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        let mut tail = Tail::new(path.clone());
        let line = |event: &str| {
            format!(r#"{{"event":"{event}","tool_name":"Bash","tool_input":null,"ts":"t"}}"#)
        };
        writeln!(f, "{}", line("UserPromptSubmit")).unwrap();
        writeln!(f, "{}", line("PreToolUse")).unwrap();
        writeln!(f, "{}", line("Stop")).unwrap();
        f.flush().unwrap();
        let polled = tail.poll();
        let got: Vec<&str> = polled.transitions.iter().map(|o| o.state).collect();
        assert_eq!(got, vec!["working", "done"]);
        assert!(polled.activity);
        let polled = tail.poll();
        assert!(
            polled.transitions.is_empty() && !polled.activity,
            "nothing new"
        );

        // A repeated `working` is activity without a transition.
        writeln!(f, "{}", line("UserPromptSubmit")).unwrap();
        writeln!(f, "{}", line("PreToolUse")).unwrap();
        f.flush().unwrap();
        let polled = tail.poll();
        assert_eq!(polled.transitions.len(), 1, "one working transition");
        writeln!(f, "{}", line("PreToolUse")).unwrap();
        f.flush().unwrap();
        let polled = tail.poll();
        assert!(
            polled.transitions.is_empty() && polled.activity,
            "{polled:?}"
        );

        // The same permission twice is one waiting; a different tool is news.
        let perm = |tool: &str| {
            format!(
                r#"{{"event":"PermissionRequest","tool_name":"{tool}","tool_input":null,"ts":"t"}}"#
            )
        };
        writeln!(f, "{}", perm("Bash")).unwrap();
        writeln!(f, "{}", perm("Bash")).unwrap();
        writeln!(f, "{}", perm("Edit")).unwrap();
        f.flush().unwrap();
        let details: Vec<String> = tail
            .poll()
            .transitions
            .into_iter()
            .map(|o| o.detail.unwrap_or_default())
            .collect();
        assert_eq!(details, vec!["permission: Bash", "permission: Edit"]);
    }

    /// The console settings carry the seven events, `*` where the ADR says,
    /// the exe quoted, and NOTHING but hooks — the operator's own settings
    /// must keep their say.
    #[test]
    fn console_settings_carry_the_hook_set_and_nothing_else() {
        let json = console_settings_json(Path::new("C:\\r\\ralphy.exe"));
        let v: Value = serde_json::from_str(&json).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.keys().collect::<Vec<_>>(), vec!["hooks"]);
        let hooks = v["hooks"].as_object().unwrap();
        let mut events: Vec<&String> = hooks.keys().collect();
        events.sort();
        assert_eq!(
            events,
            vec![
                "PermissionRequest",
                "PostToolUse",
                "PreToolUse",
                "SessionStart",
                "Stop",
                "SubagentStop",
                "UserPromptSubmit"
            ]
        );
        assert_eq!(hooks["PreToolUse"][0]["matcher"], "*");
        assert_eq!(hooks["PostToolUse"][0]["matcher"], "*");
        assert_eq!(hooks["PermissionRequest"][0]["matcher"], "*");
        assert_eq!(
            hooks["Stop"][0]["hooks"][0]["command"],
            "\"C:\\r\\ralphy.exe\" hook status"
        );
    }

    #[test]
    fn status_files_are_written_under_the_sessions_dir_and_removed_together() {
        let dir = tempfile::tempdir().unwrap();
        let files = StatusFiles::for_session(&dir.path().join("sessions"), 7);
        assert!(files.settings.ends_with("7.settings.json"));
        assert!(files.status.ends_with("7.agent-status.jsonl"));
        files.write(Path::new("ralphy")).unwrap();
        assert!(files.settings.is_file() && files.status.is_file());
        assert_eq!(std::fs::read(&files.status).unwrap(), b"");
        files.remove();
        assert!(!files.settings.exists() && !files.status.exists());
    }
}
