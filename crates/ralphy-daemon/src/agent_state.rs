//! A console's agent state from the vendor's hooks (docs/adr/0059 §5).
//!
//! The daemon writes a per-session settings file registering the §4 hook set
//! (each `<ralphy exe> hook status`), points the child at a per-session
//! status file through `RALPHY_STATUS_FILE`, and tails that file from the
//! session's own pump tick. The fold from hook line to state (ADR-0085 D7) is
//! **duplicated** here, in [`fold`] and the Claude map — the daemon never
//! imports an adapter (ADR-0032 §10) — and pinned against the adapter's
//! fixture (`crates/ralphy-agent-claude/tests/fixtures/agent_state_mapping.json`)
//! so the two cannot drift. Staleness (§6) is applied at READ time: a `working`
//! older than the interactive idle window renders `unknown`; a `waiting`
//! never goes stale; the state dies with the PTY and is never persisted.
//!
//! Nothing here reads the terminal (§7).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use ralphy_proc_util::hook_form::{HookCheck, HookCommand};
use serde_json::Value;

use crate::session::Agent;

pub mod fold;

mod claude;

use fold::{Fold, StatusLine};

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
/// `working`, `waiting`, `done`, `unknown` (ADR-0085 D1); `since` the hook's
/// timestamp; `detail` what a `waiting` agent asks. A silent console
/// (docs/adr/0059 A2) has no `state`: `source` is `silent`, `reason` says
/// why, and `since` is when the silence began.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub since: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl AgentState {
    /// The block of a silent console: no state, the reason, and when the
    /// silence began (RFC 3339, UTC).
    pub fn silent(reason: String, since: SystemTime) -> Self {
        AgentState {
            state: None,
            since: chrono::DateTime::<chrono::Utc>::from(since)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            detail: None,
            source: Some("silent".to_string()),
            reason: Some(reason),
        }
    }
}

/// How long a console with hooks may go without a hook line after the
/// operator's first input before it is silent (docs/adr/0059 A2).
pub const SILENT_AFTER: Duration = Duration::from_secs(30);

/// The reason of a console silent by the clock.
const CLOCK_REASON: &str = "No hook event reached Ralphy in the 30 seconds after your first input, so Ralphy cannot see what this agent is doing.";

/// The silence rule (docs/adr/0059 A2): the reason a console is silent, or
/// `None` when it is not. Only a console whose state comes from hooks can be
/// silent, and the first hook line ends the silence for good. A failed start
/// check makes it silent at once; otherwise it is silent once more than
/// [`SILENT_AFTER`] passed since the first input with no line.
pub fn silence(
    source: StateSource,
    first_input: Option<SystemTime>,
    first_line: Option<SystemTime>,
    start_failure: Option<&str>,
    now: SystemTime,
) -> Option<String> {
    if source != StateSource::Hooks || first_line.is_some() {
        return None;
    }
    if let Some(cause) = start_failure {
        return Some(format!(
            "The hook command failed when the daemon started: {cause}"
        ));
    }
    let waited = now.duration_since(first_input?).ok()?;
    (waited > SILENT_AFTER).then(|| CLOCK_REASON.to_string())
}

/// The final bytes a terminal reply ends with: the cursor position (`R`),
/// the device attributes (`c`), a status report (`n`), a mode report (`y`)
/// and a focus change (`I`, `O`). A key such as an arrow ends with another
/// byte.
const REPLY_FINALS: &[u8] = b"RcnyIO";

/// Whether `bytes` holds only terminal replies: the answers the browser
/// terminal sends by itself to a query of the child, such as the cursor
/// position `ESC[12;40R` or the device attributes `ESC[?1;2c`. Such a write
/// is not the operator's input, so it does not start the silence clock.
pub fn is_terminal_report(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    while !rest.is_empty() {
        let Some(len) = reply_len(rest) else {
            return false;
        };
        rest = &rest[len..];
    }
    !bytes.is_empty()
}

/// The length of the terminal reply at the start of `bytes`: `ESC [`, the
/// parameter and intermediate bytes, and a final byte of [`REPLY_FINALS`].
fn reply_len(bytes: &[u8]) -> Option<usize> {
    let body = bytes.strip_prefix(b"\x1b[")?;
    let end = body.iter().position(|b| !(0x20..=0x3f).contains(b))?;
    REPLY_FINALS.contains(&body[end]).then_some(end + 3)
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
        state: Some(if stale { "unknown" } else { obs.state }.to_string()),
        since: obs.since.clone(),
        detail: obs.detail.clone(),
        source: None,
        reason: None,
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
    fold: Fold,
}

impl Tail {
    pub fn new(path: PathBuf) -> Self {
        Tail {
            path,
            offset: 0,
            carry: Vec::new(),
            fold: Fold::new(),
        }
    }

    pub fn poll(&mut self) -> Polled {
        let mut out = Polled::default();
        for text in self.read_lines() {
            let Some(line) = StatusLine::parse(text.trim()) else {
                continue;
            };
            // Any line the map reads is proof of life, a subagent's too.
            out.activity |= claude::claude_signal(&line).is_some();
            if let Some(folded) = self.fold.apply(&line, claude::claude_signal) {
                out.transitions.push(Observed {
                    state: folded.state.as_str(),
                    detail: folded.detail,
                    since: folded.ts,
                    seen: SystemTime::now(),
                });
            }
        }
        out
    }

    // The same read is in `ralphy-adapter-support/src/agent_state.rs`, and it stays two copies: the
    // daemon imports no adapter crate (ADR-0059 §5).
    // jscpd:ignore-start
    /// The complete lines appended since the last read. A partial last line
    /// waits for its newline.
    fn read_lines(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return lines;
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return lines;
        }
        let mut buf = Vec::new();
        let Ok(n) = file.read_to_end(&mut buf) else {
            return lines;
        };
        self.offset += n as u64;
        self.carry.extend_from_slice(&buf);
        while let Some(nl) = self.carry.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.carry.drain(..=nl).collect();
            if let Ok(text) = String::from_utf8(line) {
                lines.push(text);
            }
        }
        lines
    }
    // jscpd:ignore-end
}

/// Where a console's agent state comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateSource {
    /// The vendor's own hooks, written by the daemon into a settings file.
    Hooks,
    /// The vendor gives no state.
    None,
}

/// The state source of an agent. Exhaustive, so a new agent cannot be added
/// without choosing one (docs/adr/0059 A1, docs/adr/0040 Tier 4).
pub fn state_source(agent: Agent) -> StateSource {
    match agent {
        Agent::Claude => StateSource::Hooks,
        Agent::Codex
        | Agent::Copilot
        | Agent::Cursor
        | Agent::Gemini
        | Agent::Kimi
        | Agent::OpenCode => StateSource::None,
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

    /// Write the settings file (the §4 hook set, each the `hook` entry) and an
    /// empty status file. The settings are the hooks and NOTHING
    /// else: the vendor merges `--settings` over the operator's own file, so
    /// their permissions and hooks stay (ADR-0059 §5).
    pub fn write(&self, hook: &HookCommand) -> std::io::Result<()> {
        if let Some(dir) = self.settings.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.settings, console_settings_json(hook))?;
        std::fs::write(&self.status, b"")
    }

    pub fn remove(&self) {
        // Best effort, and quiet: a session that ended has nothing to report
        // about a file the daemon itself created.
        let _settings = std::fs::remove_file(&self.settings);
        let _status = std::fs::remove_file(&self.status);
    }
}

/// The hook events of a Claude console and their matchers, the same list the
/// adapter registers (ADR-0059 §4, ADR-0085 D7), and none of the ones it
/// rejected. Pinned by the adapter's `tests/fixtures/hook_events.json`.
const CONSOLE_HOOK_EVENTS: [(&str, &str); 9] = [
    ("SessionStart", ""),
    ("UserPromptSubmit", ""),
    ("PreToolUse", "*"),
    ("PostToolUse", "*"),
    ("PermissionRequest", "*"),
    ("Elicitation", "*"),
    ("ElicitationResult", "*"),
    ("Stop", ""),
    ("SubagentStop", ""),
];

/// The console's settings document: the hook set on every event, each
/// running `hook` (built by the caller from the Claude Code version, so the
/// entry has the form that version runs).
pub fn console_settings_json(hook: &HookCommand) -> String {
    let command = hook.entry();
    let hooks: serde_json::Map<String, Value> = CONSOLE_HOOK_EVENTS
        .iter()
        .map(|(event, matcher)| {
            let entry = serde_json::json!([{
                "matcher": matcher,
                "hooks": [ command.clone() ]
            }]);
            (event.to_string(), entry)
        })
        .collect();
    let settings = serde_json::json!({ "hooks": hooks });
    serde_json::to_string_pretty(&settings).expect("settings serialize")
}

/// The hook command of a Claude console that runs `claude`: this ralphy
/// program with `hook status`, in the form the version of `claude` runs. The
/// console launch, the start check and `ralphy doctor` all use it, so the
/// check answers for the command a console gets.
pub fn console_hook(claude: &Path) -> HookCommand {
    let exe = PathBuf::from(crate::dispatch::ralphy_exe());
    ralphy_proc_util::hook_form::claude_hook(claude, &exe, &["hook", "status"])
}

/// The result of the hook check this daemon ran at start.
static START_CHECK: OnceLock<HookCheck> = OnceLock::new();

/// Run the Claude console hook command, in the form a console would get,
/// once with a test event, and keep the result. Blocking: the daemon runs it
/// on a background thread at start.
pub fn run_start_check() {
    let claude = Agent::Claude
        .locate_program()
        .unwrap_or_else(|| PathBuf::from("claude"));
    let hook = console_hook(&claude);
    let home = ralphy_proc_util::home_dir();
    let result = start_check(&hook, home.as_deref());
    // A second call keeps the first result: the check runs once per daemon.
    let _first = START_CHECK.set(result);
}

/// Run the check and log one line when it fails.
fn start_check(hook: &HookCommand, home: Option<&Path>) -> HookCheck {
    let result =
        ralphy_proc_util::hook_form::check(hook, ralphy_proc_util::hook_form::CHECK_LIMIT, home);
    if let HookCheck::Failed { cause } = &result {
        tracing::warn!("the Claude console hook command failed its check: {cause}");
    }
    result
}

/// The cause of a failed start check, or `None` when it passed, was not
/// run, or has not finished.
pub fn start_check_failure() -> Option<String> {
    match START_CHECK.get()? {
        HookCheck::Failed { cause } => Some(cause.clone()),
        HookCheck::Ok | HookCheck::NotChecked { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralphy_proc_util::hook_form::hook_for_version;

    #[test]
    fn the_start_check_runs_in_the_background_and_logs_one_line() {
        let serve: String = include_str!("serve.rs")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .replace(",)", ")");
        let call = "spawn_blocking(crate::agent_state::run_start_check)";
        let start = serve
            .find(call)
            .expect("serve starts the check with spawn_blocking");
        let statement = serve[start..].split(';').next().unwrap_or("");
        assert!(
            !statement.contains(".await"),
            "the check is not awaited: {statement}"
        );

        let home = tempfile::tempdir().expect("temp dir");
        let missing = home.path().join("no-such-ralphy");
        let hook = hook_for_version(Some("2.1.139 (Claude Code)"), &missing, &["hook", "status"]);
        let log_path = home.path().join("daemon.log");
        let log_file = std::fs::File::create(&log_path).expect("create the log");
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || log_file.try_clone().expect("a second log handle"))
            .with_ansi(false)
            .finish();
        let got =
            tracing::subscriber::with_default(subscriber, || start_check(&hook, Some(home.path())));
        assert!(matches!(got, HookCheck::Failed { .. }), "{got:?}");
        let log = std::fs::read_to_string(&log_path).expect("read the log");
        let warnings = log.lines().filter(|l| l.contains("WARN")).count();
        assert_eq!(warnings, 1, "{log}");
        assert!(log.contains("no-such-ralphy"), "{log}");
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
        assert_eq!(
            render(&obs("working"), fresh).state.as_deref(),
            Some("working")
        );
        assert_eq!(
            render(&obs("working"), old).state.as_deref(),
            Some("unknown")
        );
        assert_eq!(
            render(&obs("waiting"), old).state.as_deref(),
            Some("waiting")
        );
        assert_eq!(render(&obs("done"), old).state.as_deref(), Some("done"));
        // A clock that went backwards is not an age.
        assert_eq!(
            render(&obs("working"), t0 - Duration::from_secs(5))
                .state
                .as_deref(),
            Some("working")
        );
    }

    /// The silence rule, one row per case.
    #[test]
    fn the_silence_rule_decides_each_case() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let ago = |s: u64| Some(now - Duration::from_secs(s));
        let hooks = StateSource::Hooks;
        assert_eq!(silence(hooks, None, None, None, now), None, "no input");
        assert_eq!(silence(hooks, ago(29), None, None, now), None, "29 s");
        assert_eq!(
            silence(hooks, ago(31), None, None, now).as_deref(),
            Some(CLOCK_REASON),
            "31 s"
        );
        assert_eq!(silence(hooks, ago(31), ago(1), None, now), None, "a line");
        let start = silence(hooks, None, None, Some("boom: not found"), now)
            .expect("a failed start check is silent at once");
        assert_eq!(
            start,
            "The hook command failed when the daemon started: boom: not found"
        );
        assert_eq!(
            silence(StateSource::None, ago(31), None, None, now),
            None,
            "no hooks, no silence"
        );
    }

    /// A write the terminal sends by itself is not input; a key is.
    #[test]
    fn terminal_reports_are_told_from_keys() {
        for (bytes, report) in [
            (&b"\x1b[12;40R"[..], true),
            (b"\x1b[?1;2c", true),
            (b"\x1b[?1;2c\x1b[12;40R", true),
            (b"\x1b[I", true),
            (b"hello", false),
            (b"\x1b[B\r", false),
            (b"\x1b[B", false),
            (b"\x1b[12;40", false),
            (b"", false),
        ] {
            assert_eq!(is_terminal_report(bytes), report, "{bytes:?}");
        }
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

    const EXE: &str = if cfg!(windows) {
        r"C:\r\ralphy.exe"
    } else {
        "/r/ralphy"
    };

    fn hook_of(version: &str) -> HookCommand {
        hook_for_version(Some(version), Path::new(EXE), &["hook", "status"])
    }

    /// The console settings carry the adapter's shared event list, with its
    /// matchers, and NOTHING but hooks: the operator's own settings must keep
    /// their say.
    #[test]
    fn console_settings_match_the_shared_event_list() {
        let shared: Vec<Value> = serde_json::from_str(include_str!(
            "../../ralphy-agent-claude/tests/fixtures/hook_events.json"
        ))
        .unwrap();
        let mut want: Vec<(String, String)> = shared
            .iter()
            .map(|e| {
                (
                    e["event"].as_str().unwrap().to_string(),
                    e["matcher"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        want.sort();
        let hook = hook_of("2.1.139");
        let json = console_settings_json(&hook);
        let v: Value = serde_json::from_str(&json).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.keys().collect::<Vec<_>>(), vec!["hooks"]);
        let hooks = v["hooks"].as_object().unwrap();
        let mut got: Vec<(String, String)> = hooks
            .iter()
            .map(|(event, list)| {
                assert_eq!(list.as_array().map(Vec::len), Some(1), "{event}");
                assert_eq!(list[0]["hooks"][0], hook.entry(), "{event}");
                (
                    event.clone(),
                    list[0]["matcher"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        got.sort();
        assert_eq!(got, want);
        for event in ["Elicitation", "ElicitationResult"] {
            assert!(hooks.contains_key(event), "{event}");
        }
    }

    /// Below 2.1.139 the entry is one shell string; from it, the program and
    /// its arguments apart.
    #[test]
    fn console_settings_use_the_form_of_the_claude_version() {
        let old: Value = serde_json::from_str(&console_settings_json(&hook_of("2.1.138"))).unwrap();
        let shell = old["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(shell.ends_with(" hook status"), "{shell}");
        assert!(old["hooks"]["Stop"][0]["hooks"][0]["args"].is_null());

        let new: Value = serde_json::from_str(&console_settings_json(&hook_of("2.1.139"))).unwrap();
        assert_eq!(
            new["hooks"]["Stop"][0]["hooks"][0],
            serde_json::json!({"type": "command", "command": EXE, "args": ["hook", "status"]})
        );
    }

    /// A daemon started through a canonicalized path runs as `\\?\C:\...`;
    /// Git Bash, which runs the hooks, cannot run that form.
    #[cfg(windows)]
    #[test]
    fn console_hooks_name_the_binary_without_the_verbatim_prefix() {
        let hook = hook_for_version(
            Some("2.1.139"),
            Path::new(r"\\?\C:\r\ralphy.exe"),
            &["hook", "status"],
        );
        let doc: Value = serde_json::from_str(&console_settings_json(&hook)).unwrap();
        assert_eq!(
            doc["hooks"]["Stop"][0]["hooks"][0]["command"],
            r"C:\r\ralphy.exe"
        );
    }

    /// Claude has hooks; no other agent gives a state. The match names every
    /// variant, so a new agent has to choose.
    #[test]
    fn every_agent_has_a_state_source() {
        for agent in Agent::ALL {
            let want = if agent == Agent::Claude {
                StateSource::Hooks
            } else {
                StateSource::None
            };
            assert_eq!(state_source(agent), want, "{agent:?}");
        }
        let src = include_str!("agent_state.rs");
        let start = src.find("pub fn state_source(").unwrap();
        let end = start + src[start..].find("\n}").unwrap();
        let body = &src[start..end];
        assert!(!body.contains("_ =>"), "the match must name every agent");
    }

    #[test]
    fn status_files_are_written_under_the_sessions_dir_and_removed_together() {
        let dir = tempfile::tempdir().unwrap();
        let files = StatusFiles::for_session(&dir.path().join("sessions"), 7);
        assert!(files.settings.ends_with("7.settings.json"));
        assert!(files.status.ends_with("7.agent-status.jsonl"));
        files.write(&hook_of("2.1.139")).unwrap();
        assert!(files.settings.is_file() && files.status.is_file());
        assert_eq!(std::fs::read(&files.status).unwrap(), b"");
        files.remove();
        assert!(!files.settings.exists() && !files.status.exists());
    }
}
