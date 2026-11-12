//! A console's agent state from the vendor's hooks (docs/adr/0059 §5).
//!
//! The daemon writes a per-session settings file registering the §4 hook set
//! (each `<ralphy exe> hook status`), points the child at a per-session
//! status file through `RALPHY_STATUS_FILE`, and tails that file from the
//! session's own pump tick. A Codex console takes its hooks inline, as `-c`
//! arguments, and gets no settings file. An `OpenCode` console gets a state
//! plugin file instead, named in `OPENCODE_CONFIG_CONTENT`, which appends to
//! the same kind of status file (ADR-0085 D2). The fold from hook line to state
//! (ADR-0085 D7) is **duplicated** here, in [`fold`] and the vendor maps —
//! the daemon never imports an adapter (ADR-0032 §10) — and pinned against
//! each adapter's fixture (`tests/fixtures/agent_state_mapping.json`) so the
//! two cannot drift. Staleness (§6) is applied at READ time: a `working`
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
mod codex;
pub(crate) mod opencode;
mod status_files;

pub use status_files::StatusFiles;

use fold::{Fold, MapFn, StatusLine};

/// The idle window a `working` may age past before it renders `unknown` —
/// the interactive default `ralphy-core` reaps a wedged run at
/// (`DEFAULT_INTERACTIVE_IDLE_MINUTES`), restated here because the daemon
/// does not import core. A console has no reaper; this is only how long a
/// green dot is believed.
pub const STALE_AFTER: Duration = Duration::from_mins(45);

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
    /// Why a `done` is `done`; `None` on any other state.
    pub cause: Option<fold::Cause>,
}

/// The wire shape on `/api/sessions` and the fleet: `state` is one of
/// `working`, `waiting`, `done`, `unknown` (ADR-0085 D1); `since` the hook's
/// timestamp; `detail` what a `waiting` agent asks. A console whose state
/// is not reported has no `state`: `source` says why ([`Unreported`]),
/// `reason` says it in words, and `since` is when it began.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub since: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Unreported>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Why a console's agent state is not reported. Matched exhaustively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Unreported {
    /// The console has a state source and it sent nothing (docs/adr/0059
    /// A2): the console is **silent**, an attention request.
    #[serde(rename = "silent")]
    Silent,
    /// The launch gave the console no state source on purpose. It is not
    /// an attention request: it would stay open for the console's whole life.
    #[serde(rename = "none")]
    NoSource,
}

impl AgentState {
    /// The block of a silent console: no state, the reason, and when the
    /// silence began (RFC 3339, UTC).
    pub fn silent(reason: String, since: SystemTime) -> Self {
        Self::unreported(Unreported::Silent, reason, since)
    }

    /// The block of a console whose state is not reported, for `why`: no
    /// state, the reason, and when it began (RFC 3339, UTC).
    pub fn unreported(why: Unreported, reason: String, since: SystemTime) -> Self {
        AgentState {
            state: None,
            since: chrono::DateTime::<chrono::Utc>::from(since)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            detail: None,
            source: Some(why),
            reason: Some(reason),
        }
    }
}

/// How long a console with hooks may go without a hook line after the
/// operator's first input before it is silent (docs/adr/0059 A2).
pub const SILENT_AFTER: Duration = Duration::from_secs(30);

/// The reason of a console silent by the clock.
const CLOCK_REASON: &str = "No hook event reached Ralphy in the 30 seconds after \
    your first input, so Ralphy cannot see what this agent is doing.";

/// The reason of a Codex console in a project with Codex hooks of its own:
/// it is launched without the status hooks (ADR-0085 amendment of
/// 2026-10-29).
pub const CODEX_OWN_HOOKS_REASON: &str =
    "This project has its own Codex hooks, so Ralphy does not show the agent state here.";

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
    (waited > SILENT_AFTER).then(|| CLOCK_REASON.to_owned())
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
        state: Some(if stale { "unknown" } else { obs.state }.to_owned()),
        since: obs.since.clone(),
        detail: obs.detail.clone(),
        source: None,
        reason: None,
    }
}

/// What one fold of the read lines saw: the state TRANSITIONS, in order,
/// and whether ANY line folded — a repeated `working` is not a transition
/// but it is proof of life, and the staleness clock (§6) must read it.
#[derive(Debug, Default)]
pub struct Polled {
    pub transitions: Vec<Observed>,
    pub activity: bool,
}

/// The reader of a status file: the complete lines appended since its last
/// read. It folds nothing, so it can read while no lock on the state is held.
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    offset: u64,
    carry: Vec<u8>,
}

impl Tail {
    pub fn new(path: PathBuf) -> Self {
        Tail {
            path,
            offset: 0,
            carry: Vec::new(),
        }
    }

    // The same read is in `ralphy-adapter-support/src/agent_state.rs`, and it stays two copies: the
    // daemon imports no adapter crate (ADR-0059 §5).
    // jscpd:ignore-start
    /// The complete lines appended since the last read. A partial last line
    /// waits for its newline.
    pub fn read_lines(&mut self) -> Vec<String> {
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
        self.offset += u64::try_from(n).unwrap_or(u64::MAX);
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

/// The observation of one folded state, seen at `seen`.
fn observed(folded: fold::Observed, seen: SystemTime) -> Observed {
    Observed {
        state: folded.state.as_str(),
        detail: folded.detail,
        since: folded.ts,
        seen,
        cause: folded.cause,
    }
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
        Agent::Claude | Agent::Codex | Agent::OpenCode => StateSource::Hooks,
        Agent::Copilot | Agent::Cursor | Agent::Gemini | Agent::Kimi => StateSource::None,
    }
}

/// A vendor whose hooks fill a console's status file, and how its lines fold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookVendor {
    Claude,
    Codex,
    /// `OpenCode`, whose state plugin writes the lines (ADR-0085 D2).
    OpenCode,
}

impl HookVendor {
    /// The vendor of `agent`, when its state comes from hooks.
    pub fn of(agent: Agent) -> Option<HookVendor> {
        match agent {
            Agent::Claude => Some(HookVendor::Claude),
            Agent::Codex => Some(HookVendor::Codex),
            Agent::OpenCode => Some(HookVendor::OpenCode),
            Agent::Copilot | Agent::Cursor | Agent::Gemini | Agent::Kimi => None,
        }
    }

    /// Fold `lines` of this vendor, read at `now` from a [`Tail`], into
    /// `fold`, and report the transitions — a change of state, or a
    /// `waiting` with a new detail (a second question is news; the same
    /// question re-asked is not).
    pub fn fold_lines(self, fold: &mut Fold, lines: &[String], now: SystemTime) -> Polled {
        let mut out = Polled::default();
        for text in lines {
            let Some(line) = StatusLine::parse(text.trim()) else {
                continue;
            };
            let map = self.map();
            // Any line the map reads is proof of life, a subagent's too, and
            // so is a plugin's first line.
            out.activity |= map(&line).is_some() || self.is_first_line(&line);
            if let Some(folded) = fold.apply(&line, map) {
                out.transitions.push(observed(folded, now));
            }
        }
        out
    }

    /// The operator wrote to the console at `now` (ADR-0085 D7.5). For a
    /// vendor that sends no event when the operator grants a permission, an
    /// open permission `waiting` in `fold` ends here; the result is the new
    /// state, else `None`.
    pub fn fold_input(self, fold: &mut Fold, now: SystemTime) -> Option<Observed> {
        if self.sends_a_grant() {
            return None;
        }
        let ts = chrono::DateTime::<chrono::Utc>::from(now)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        Some(observed(fold.operator_input(ts)?, now))
    }

    fn map(self) -> MapFn {
        match self {
            HookVendor::Claude => claude::claude_signal,
            HookVendor::Codex => codex::launched,
            HookVendor::OpenCode => opencode::opencode_signal,
        }
    }

    /// Whether `line` is the first line a vendor's plugin writes when it
    /// loads: it changes no state, but it proves that the source runs
    /// (ADR-0085 D9).
    fn is_first_line(self, line: &StatusLine) -> bool {
        match self {
            HookVendor::OpenCode => line.event == opencode::PLUGIN_LOADED,
            HookVendor::Claude | HookVendor::Codex => false,
        }
    }

    /// Whether the vendor's hook runs the ralphy program (`ralphy hook
    /// status`), so that the daemon's start check of that program speaks for
    /// it. `OpenCode`'s plugin runs in `OpenCode` and never starts it.
    pub(crate) fn runs_the_ralphy_program(self) -> bool {
        match self {
            HookVendor::Claude | HookVendor::Codex => true,
            HookVendor::OpenCode => false,
        }
    }

    /// Whether the vendor sends an event when the operator grants a
    /// permission (ADR-0085 D7.5): Claude sends `PostToolUse` as the tool
    /// runs, `OpenCode` sends `permission.replied`; Codex sends nothing until
    /// the tool ends.
    fn sends_a_grant(self) -> bool {
        match self {
            HookVendor::Claude | HookVendor::OpenCode => true,
            HookVendor::Codex => false,
        }
    }
}

/// The arguments that give a Codex console its status hooks (ADR-0085 D2):
/// each hook runs this ralphy program with `hook status codex`.
pub fn codex_console_hook_args() -> Vec<String> {
    let exe = PathBuf::from(crate::dispatch::ralphy_exe());
    ralphy_proc_util::hook_form::codex_hook_args(
        &exe,
        &ralphy_proc_util::hook_form::CODEX_STATUS_EVENTS,
    )
}

/// The command that checks the hook of a Codex console: the program and the
/// arguments its hooks run. `ralphy doctor` runs it.
pub fn codex_check_hook() -> HookCommand {
    let exe = PathBuf::from(crate::dispatch::ralphy_exe());
    ralphy_proc_util::hook_form::codex_check_hook(&exe)
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
///
/// # Panics
///
/// Never: a JSON value of string keys always serializes.
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
mod tests;
