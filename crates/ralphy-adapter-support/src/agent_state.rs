//! The agent state from a status file that the vendor fills (ADR-0085 D2, D7,
//! D8): the parsed status line, the fold with memory that applies the D7
//! rules, and the tail and watcher that read the file while a child runs.
//!
//! The fold is vendor-neutral. Each adapter gives a map from one status line
//! to one [`Signal`]; the fold keeps the lead session, the state a `waiting`
//! replaced, and the tool use the `waiting` is for. The daemon keeps its own
//! copy of this fold (it imports no adapter crate, ADR-0059 §5), and the
//! vendor's shared mapping fixture pins the two copies equal.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Context as _;
use serde_json::Value;

// The daemon has the same engine in `ralphy-daemon/src/agent_state/fold.rs`,
// and it stays two copies: the daemon imports no adapter crate (ADR-0059 §5).
// Each vendor's shared mapping fixture holds the two copies equal.
// jscpd:ignore-start
/// One line of the status file, as `ralphy hook status` (or a vendor plugin)
/// writes it (ADR-0085 D8). A field the vendor did not send is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusLine {
    pub event: String,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    pub question: Option<String>,
    pub notification_type: Option<String>,
    pub ts: String,
}

impl StatusLine {
    /// The line parsed from its JSON text, or `None` when the text is not a
    /// JSON object.
    pub fn parse(text: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(text).ok()?;
        v.as_object()?;
        let field = |name: &str| {
            v.get(name)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Some(StatusLine {
            event: field("event").unwrap_or_default(),
            session_id: field("session_id"),
            agent_id: field("agent_id"),
            tool_name: field("tool_name"),
            tool_use_id: field("tool_use_id"),
            question: field("question"),
            notification_type: field("notification_type"),
            ts: field("ts").unwrap_or_default(),
        })
    }
}

/// The state a fold gives. `unknown` (ADR-0085 D1) is not here: it is a
/// `working` that aged past the idle window, and only a reader makes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Working,
    Waiting,
    Done,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Working => "working",
            State::Waiting => "waiting",
            State::Done => "done",
        }
    }
}

/// What a `waiting` waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    /// The agent asks the operator a question.
    Question,
    /// The vendor asks the operator to allow a tool.
    Permission,
    /// An MCP server asks the operator for input.
    Elicitation,
}

/// What one status line means to the fold. A vendor's map gives one, or
/// `None` for a line that changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// A tool starts: the agent works.
    Work,
    /// The operator sent a prompt: a turn starts, and any `waiting` ends.
    TurnStart,
    /// The agent waits for the operator. `tool_use_id` is the tool use the
    /// wait is for, when the vendor names it.
    Wait {
        kind: WaitKind,
        tool_use_id: Option<String>,
        detail: String,
    },
    /// A wait may end here (a tool returned, an answer came). It ends the
    /// `waiting` only when it is for the same tool use (D7.3).
    WaitEnd { tool_use_id: Option<String> },
    /// The turn ended: the agent is idle at its prompt.
    TurnEnd,
    /// A session started: the agent is idle at its prompt, and this
    /// session is the lead from now on.
    SessionBoundary,
    /// The operator interrupted the turn (D7.4).
    Interrupt,
}

/// Why a `done` is `done`: a turn that ended is not a session that started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    TurnEnd,
    SessionBoundary,
    Interrupt,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Cause::TurnEnd => "turn-end",
            Cause::SessionBoundary => "session-boundary",
            Cause::Interrupt => "interrupt",
        }
    }
}

/// One state change, folded from one status line. `ts` is the line's own
/// timestamp; `cause` is set on a `done` only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub state: State,
    pub detail: Option<String>,
    pub ts: String,
    pub cause: Option<Cause>,
}

/// A vendor's map from one status line to one signal.
pub type MapFn = fn(&StatusLine) -> Option<Signal>;

/// The state the fold shows, without the time.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    state: State,
    detail: Option<String>,
    cause: Option<Cause>,
}

impl Shown {
    fn working() -> Self {
        Shown {
            state: State::Working,
            detail: None,
            cause: None,
        }
    }

    fn done(cause: Cause) -> Self {
        Shown {
            state: State::Done,
            detail: None,
            cause: Some(cause),
        }
    }
}

/// An open `waiting`: what it waits for, and the state it replaced (D7.2).
#[derive(Debug, Clone)]
struct Wait {
    kind: WaitKind,
    tool_use_id: Option<String>,
    tool_name: Option<String>,
    replaced: Option<Shown>,
}

impl Wait {
    /// Whether a wait end on `line` with `tool_use_id` ends this wait. Two
    /// known tool use ids decide (D7.3); else two known tool names; a
    /// question of an MCP server ends only at a line with no tool.
    fn ends_at(&self, line: &StatusLine, tool_use_id: Option<&String>) -> bool {
        if let (Some(mine), Some(theirs)) = (&self.tool_use_id, tool_use_id) {
            return mine == theirs;
        }
        match (&self.tool_name, &line.tool_name) {
            (Some(mine), Some(theirs)) => mine == theirs,
            (None, Some(_)) => self.kind != WaitKind::Elicitation,
            (_, None) => true,
        }
    }
}

/// The fold with memory (ADR-0085 D7): the lead session, the state shown,
/// and the open `waiting`.
#[derive(Debug, Default)]
pub struct Fold {
    lead: Option<String>,
    current: Option<Shown>,
    wait: Option<Wait>,
}

impl Fold {
    pub fn new() -> Self {
        Fold::default()
    }

    /// What the open `waiting` waits for, or `None` when there is none.
    pub fn waiting(&self) -> Option<WaitKind> {
        self.wait.as_ref().map(|w| w.kind)
    }

    /// Fold one line. The result is the new state when the line changed the
    /// state or its detail, else `None`.
    pub fn apply(&mut self, line: &StatusLine, map: MapFn) -> Option<Observed> {
        let signal = map(line)?;
        if self.lead.is_none() && line.agent_id.is_none() {
            self.lead.clone_from(&line.session_id);
        }
        let next = self.next(line, signal)?;
        let same = self
            .current
            .as_ref()
            .is_some_and(|c| c.state == next.state && c.detail == next.detail);
        self.current = Some(next.clone());
        (!same).then(|| Observed {
            state: next.state,
            detail: next.detail,
            ts: line.ts.clone(),
            cause: next.cause,
        })
    }

    /// Whether `line` comes from the lead session (D7.1). A line with an
    /// `agent_id` is a subagent's; a line with no `session_id` is the lead's.
    fn is_lead(&self, line: &StatusLine) -> bool {
        if line.agent_id.is_some() {
            return false;
        }
        match (&self.lead, &line.session_id) {
            (Some(lead), Some(session)) => lead == session,
            _ => true,
        }
    }

    fn next(&mut self, line: &StatusLine, signal: Signal) -> Option<Shown> {
        match signal {
            // A subagent's wait is the lead's wait (D7.1).
            Signal::Wait {
                kind,
                tool_use_id,
                detail,
            } => Some(self.begin_wait(line, kind, tool_use_id, detail)),
            Signal::WaitEnd { tool_use_id } => self.end_wait(line, tool_use_id.as_ref()),
            Signal::SessionBoundary if line.agent_id.is_none() => {
                self.lead.clone_from(&line.session_id);
                Some(self.close(Shown::done(Cause::SessionBoundary)))
            }
            _ if !self.is_lead(line) => None,
            Signal::Work => self.work(),
            Signal::TurnStart => Some(self.close(Shown::working())),
            Signal::TurnEnd => Some(self.close(Shown::done(Cause::TurnEnd))),
            Signal::Interrupt => Some(self.close(Shown::done(Cause::Interrupt))),
            Signal::SessionBoundary => None,
        }
    }

    /// End any open wait and show `shown`.
    fn close(&mut self, shown: Shown) -> Shown {
        self.wait = None;
        shown
    }

    fn begin_wait(
        &mut self,
        line: &StatusLine,
        kind: WaitKind,
        tool_use_id: Option<String>,
        detail: String,
    ) -> Shown {
        // A second wait keeps the state the first one replaced.
        let replaced = match self.wait.take() {
            Some(open) => open.replaced,
            None => self.current.clone(),
        };
        self.wait = Some(Wait {
            kind,
            tool_use_id,
            tool_name: line.tool_name.clone(),
            replaced,
        });
        Shown {
            state: State::Waiting,
            detail: Some(detail),
            cause: None,
        }
    }

    fn end_wait(&mut self, line: &StatusLine, tool_use_id: Option<&String>) -> Option<Shown> {
        match &self.wait {
            None => self.is_lead(line).then(Shown::working),
            Some(open) if open.ends_at(line, tool_use_id) => {
                let replaced = self.wait.take().and_then(|w| w.replaced);
                Some(replaced.unwrap_or_else(Shown::working))
            }
            // Another tool returned: the wait stays (D7.3).
            Some(_) => None,
        }
    }

    /// A tool starts. Behind an open wait the state stays `waiting`, and the
    /// wait now gives back `working` when it ends.
    fn work(&mut self) -> Option<Shown> {
        match self.wait.as_mut() {
            Some(open) => {
                open.replaced = Some(Shown::working());
                None
            }
            None => Some(Shown::working()),
        }
    }
}
// jscpd:ignore-end

/// The tail: reads what was appended since the last poll and folds each
/// complete line through `map`.
pub struct Tail {
    path: PathBuf,
    offset: u64,
    carry: Vec<u8>,
    fold: Fold,
    map: MapFn,
}

impl Tail {
    pub fn new(path: PathBuf, map: MapFn) -> Self {
        Tail {
            path,
            offset: 0,
            carry: Vec::new(),
            fold: Fold::new(),
            map,
        }
    }

    /// The states that changed since the last poll, in order. A `waiting`
    /// with a new detail counts as a change (a second question is news); the
    /// same state with the same detail does not.
    pub fn poll(&mut self) -> Vec<Observed> {
        let mut out = Vec::new();
        for text in self.read_lines() {
            if let Some(line) = StatusLine::parse(text.trim()) {
                out.extend(self.fold.apply(&line, self.map));
            }
        }
        out
    }

    // The same read is in `ralphy-daemon/src/agent_state.rs`, and it stays two copies: the
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

/// Tails a status file from a thread for as long as a child runs and emits
/// each state change. `start` truncates the file (a previous child's lines
/// are not this child's); `stop` takes the last look and joins. A file that
/// cannot be truncated fails `start`.
pub struct Watcher {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    path: PathBuf,
}

impl Watcher {
    pub fn start(path: PathBuf, map: MapFn) -> anyhow::Result<Self> {
        std::fs::write(&path, b"").with_context(|| format!("resetting {}", path.display()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let tail = Tail::new(path.clone(), map);
        let handle = std::thread::spawn(move || watch(tail, &flag));
        Ok(Watcher {
            stop,
            handle: Some(handle),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn stop(mut self) {
        self.join();
    }

    fn join(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            // A panicked watcher thread has nothing to report; the run goes on.
            let _joined = h.join();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.join();
    }
}

/// The watcher thread: poll every 500 ms and emit each change until `stop`
/// is set.
fn watch(mut tail: Tail, stop: &AtomicBool) {
    loop {
        let last = stop.load(Ordering::SeqCst);
        // After the stop, one last look: a turn end lands right before the
        // child exits, and the loop must not lose it to the join.
        for obs in tail.poll() {
            emit(&obs);
        }
        if last {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn emit(obs: &Observed) {
    ralphy_core::emit::agent_state(obs.state.as_str(), &obs.ts, obs.detail.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map for a vendor that sends an interrupt event (D7.4).
    fn interrupting(line: &StatusLine) -> Option<Signal> {
        match line.event.as_str() {
            "Prompt" => Some(Signal::TurnStart),
            "Ask" => Some(Signal::Wait {
                kind: WaitKind::Permission,
                tool_use_id: Some("u1".into()),
                detail: "permission: Bash".into(),
            }),
            "Return" => Some(Signal::WaitEnd {
                tool_use_id: Some("u1".into()),
            }),
            "Interrupt" => Some(Signal::Interrupt),
            _ => None,
        }
    }

    /// D7.4: an interrupt is `done`, with its own cause, and it ends the wait.
    #[test]
    fn an_interrupt_is_done_and_ends_the_wait() {
        let mut fold = Fold::new();
        let line = |event: &str| StatusLine {
            event: event.into(),
            ts: event.into(),
            ..StatusLine::default()
        };
        let mut seen = Vec::new();
        for event in ["Prompt", "Ask", "Interrupt", "Return"] {
            let got = fold.apply(&line(event), interrupting);
            seen.push((got.map(|o| (o.state, o.cause)), fold.waiting()));
        }
        let permission = Some(WaitKind::Permission);
        assert_eq!(
            seen,
            [
                (Some((State::Working, None)), None),
                (Some((State::Waiting, None)), permission),
                (Some((State::Done, Some(Cause::Interrupt))), None),
                (Some((State::Working, None)), None),
            ]
        );
    }
}
