//! The workbench session manager (docs/adr/0032 §2): a deep module that turns
//! session verbs — spawn, write, resize, close — into a live PTY child and a
//! byte stream, knowing nothing about the HTTP transport that carries those
//! bytes (the socket bridge lives in `routes/ws_session/bridge.rs`). Keeping it transport-free is
//! what lets it be tested against a helper bin with no socket
//! (`tests/session_roundtrip.rs`) and guarded by `tests/session_transport_free.rs`.
//!
//! The blocking PTY reader is bridged to async the way ADR-0032 prescribes: a
//! `std::thread` drains the master and forwards each chunk over an unbounded
//! channel, so a sync read never blocks the tokio runtime and the send never
//! blocks the reader.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread::JoinHandle;

use anyhow::Result;
use ralphy_pty::{PtyCommand, PtySession};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Notify;

mod manager;
mod spec;

pub use manager::{AttachError, Attachment, SessionManager};
pub use spec::{
    claude_console_named, console_cwd, console_name, console_spec, cursor_indexing_allowed,
    gemini_home, gemini_policy_path, peer_console_launcher, peer_console_spec, spec_for,
    spec_with_status, Agent, SessionSpec,
};

/// Variables that suppress colour, dropped from every PTY child's inherited
/// environment.
///
/// They are the launching shell speaking for ITS pipes, not for ours, and the
/// daemon is routinely started from somewhere that has spoken: an agent's shell
/// tool exports `NO_COLOR=1`, so a daemon launched from one hands every console
/// a monochrome agent — the vendor TUI renders, but with no colour at all. The
/// daemon is a terminal, not a pipe, so it must not pass that judgement on.
/// `FORCE_COLOR` goes with it because `FORCE_COLOR=0` is the same suppression
/// spelled the other way (and any positive value is redundant beside
/// [`TERMINAL_ENV`]).
const COLOUR_SUPPRESSORS: [&str; 2] = ["NO_COLOR", "FORCE_COLOR"];

/// What the daemon declares itself to be, to every PTY child.
///
/// The consumer on the other end of these bytes is a real xterm — xterm.js with
/// truecolor — so this is a description, not a lie, and it is the daemon's to
/// make: a child that has to guess from whatever env the daemon inherited
/// renders differently depending on how the operator happened to start the
/// daemon. On Windows this neither adds nor removes capability (a child there
/// decides on the OS build), it just stops `TERM=dumb` from arriving.
const TERMINAL_ENV: [(&str, &str); 2] = [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")];

/// A live workbench session: the PTY child, the reader thread draining its
/// output, and the async channel that thread feeds. Drop or [`close`] it to tear
/// the child tree down.
///
/// [`close`]: Session::close
pub struct Session {
    // `Option` so `close` can drop the master (closing the pseudo-terminal): on
    // Windows ConPTY the output pipe only reaches EOF once the master is dropped,
    // not merely when the child dies, so the reader thread would otherwise block
    // forever after a tree kill.
    pty: Option<PtySession>,
    // Kept so the thread is owned by the session; it exits on PTY EOF (after a
    // `close` tree-kill + master drop) and is detached on drop.
    _reader: JoinHandle<()>,
    output: Option<UnboundedReceiver<Vec<u8>>>,
}

impl Session {
    /// Spawn the child in its PTY and start forwarding its output. The reader
    /// runs on a dedicated `std::thread` (a blocking read must not sit on the
    /// tokio runtime); each chunk is sent non-blocking over the unbounded channel.
    pub fn spawn(spec: SessionSpec) -> Result<Session> {
        // The same repair the headless runner applies (`program_dir_on_path`),
        // expressed against this builder rather than a `std::process::Command`:
        // a CLI resolved off `PATH` — nvm's global bin, `~/.local/bin` — may need
        // its interpreter from that very directory, since an npm shim's
        // `#!/usr/bin/env node` searches `PATH` and nvm keeps `node` beside the
        // shim. Without it an interactive session dies the moment it starts, on a
        // program the roster just reported as available (#353).
        let program_dir = PathBuf::from(&spec.program)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf);
        // A `PATH` this spec declares is the base — a vendor's own scrub still
        // wins; otherwise the child would inherit ours, so that is what we widen.
        let base_path = spec
            .env
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
            .map(|(_, v)| std::ffi::OsString::from(v))
            .or_else(|| std::env::var_os("PATH"));
        let widened =
            program_dir.and_then(|dir| ralphy_proc_util::path_with_dir(&dir, base_path.as_deref()));

        let mut cmd = PtyCommand::new(spec.program)
            .args(spec.args)
            .cwd(&spec.cwd)
            .size(spec.rows, spec.cols);
        // The terminal the daemon gives the child is described by the daemon,
        // never inherited from the shell that started it: drop the suppressors
        // first, then declare. Both come BEFORE `spec.env`, so a vendor's own
        // containment still has the last word on any key it names.
        for key in COLOUR_SUPPRESSORS {
            cmd = cmd.env_remove(key);
        }
        for (key, value) in TERMINAL_ENV {
            cmd = cmd.env(key, value);
        }
        for (k, v) in spec.env {
            cmd = cmd.env(k, v);
        }
        if let Some(path) = widened {
            cmd = cmd.env("PATH", path);
        }
        let pty = PtySession::spawn(cmd)?;
        let mut reader = pty.reader()?;
        let (tx, rx): (UnboundedSender<Vec<u8>>, UnboundedReceiver<Vec<u8>>) = unbounded_channel();
        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break, // EOF (tree exited) or a broken master
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break; // the consumer went away
                        }
                    }
                }
            }
        });
        Ok(Session {
            pty: Some(pty),
            _reader: reader_thread,
            output: Some(rx),
        })
    }

    /// Take the output receiver. Callable once — the consumer (the WS loop or a
    /// test) owns the receiver so `select!`ing on it does not borrow the session.
    pub fn take_output(&mut self) -> UnboundedReceiver<Vec<u8>> {
        self.output.take().expect("output receiver taken once")
    }

    /// Feed raw bytes to the child as terminal input. A no-op once closed.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        match self.pty.as_mut() {
            Some(pty) => pty.write_all(bytes),
            None => Ok(()),
        }
    }

    /// Whether the child has already exited (non-blocking). The pump polls this
    /// so a SELF-exited child ends the session: on Windows ConPTY the output pipe
    /// EOFs only when the master is dropped, NOT when the child dies, so the reader
    /// would otherwise never end after a `quit`. A closed session counts as exited.
    pub fn has_exited(&mut self) -> bool {
        match self.pty.as_mut() {
            Some(pty) => pty
                .try_wait()
                .map(|status| status.is_some())
                .unwrap_or(true),
            None => true,
        }
    }

    /// Resize the PTY window so the child's TUI reflows. A no-op once closed.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        match self.pty.as_ref() {
            Some(pty) => pty.resize(rows, cols),
            None => Ok(()),
        }
    }

    /// Terminate the child's whole process tree and close the PTY. A plain
    /// direct-child kill would leave a grandchild holding the PTY slave open, so
    /// kill by pid across the tree; then drop the master so ConPTY (Windows) and
    /// the slave (Unix) both reach EOF, ending the reader thread — its sender
    /// drops and the output channel yields `None`. Idempotent.
    pub fn close(&mut self) {
        if let Some(mut pty) = self.pty.take() {
            if let Some(pid) = pty.process_id() {
                ralphy_proc_util::kill_tree_by_pid(pid);
            }
            let _ = pty.kill();
            // Explicit for intent; the drop at scope end closes the master.
            drop(pty);
        }
    }
}

impl Drop for Session {
    /// Honor the type's contract — dropping a session tears its child tree down —
    /// so a consumer that never calls `close` still cannot leak a process tree.
    /// `close` is idempotent, so an explicit `close()` before drop is harmless.
    fn drop(&mut self) {
        self.close();
    }
}

/// A daemon-owned session id. `u64` fits the codec's `Frame::Terminal { session }`
/// field directly and is monotonic within a daemon lifetime, so ids never collide.
pub type SessionId = u64;

/// The identity of a live session as the UI lists it: which repo and agent, what
/// kind, and when it started. `kind` is `"agent"` for the curated launcher and
/// `"console"` for the free console (issue #167, PRD #157 story 11).
#[derive(Clone, serde::Serialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub repo: String,
    pub agent: String,
    pub kind: String,
    pub started_at: u64,
    /// Effective environment when it differs from the hosting daemon.
    pub environment: Option<String>,
    /// The display name the child was launched under, when its vendor takes one
    /// ([`spec_for`] — Claude today, nobody else). Carried here so a REATTACH can
    /// announce the same name a fresh launch did: the bridge has this record and
    /// not the spec.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The worktree NAME the console was spawned in (ADR-0063 §3), `None` for
    /// the primary tree. Announced on every `session-open`, a reattach included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
    /// The agent's own hook-reported state (ADR-0059 §5), rendered at READ
    /// time with the §6 staleness rule; absent for a vendor without hooks,
    /// before the first hook fired, and always on the record as stored —
    /// [`SessionManager::list`]/[`SessionManager::get`] fill it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_state: Option<crate::agent_state::AgentState>,
}

/// Why an attachment ended, as the bridge announces it to the client BEFORE the
/// socket closes (issue #334). The close metadata cannot carry this: a browser
/// reports `1005 / wasClean=false` even for a Close frame the daemon did send,
/// so meaning placed there is meaning lost — the reason travels in a data frame
/// instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// Another client claimed the writer slot; the session lives on elsewhere.
    TakenOver,
    /// The child exited, or the session was closed (which tree-kills it).
    ChildExited,
    /// The daemon itself is going down.
    DaemonShutdown,
}

impl EndReason {
    /// The wire word the client switches on. Fixed vocabulary — three reasons,
    /// no more (issue #334).
    pub fn as_wire(self) -> &'static str {
        match self {
            EndReason::TakenOver => "taken-over",
            EndReason::ChildExited => "child-exited",
            EndReason::DaemonShutdown => "daemon-shutdown",
        }
    }
}

/// The eviction signal one bridge waits on, carrying WHY it fired. The token is
/// the only object spanning the firing side (a takeover, a close, the pump's
/// EOF) and the waiting side (the bridge loop), so the reason has to live here
/// for the bridge to be able to announce it.
pub struct EvictToken {
    pub notify: Notify,
    reason: Mutex<Option<EndReason>>,
}

impl EvictToken {
    fn new() -> Self {
        Self {
            notify: Notify::new(),
            reason: Mutex::new(None),
        }
    }

    /// Record the reason, THEN wake. The order is load-bearing: `notify_one`
    /// stores a permit when the waiter is momentarily not parked, so the wake can
    /// be observed at any later poll — and every such poll must already see the
    /// reason, or the bridge would announce a deliberate end it cannot name.
    fn fire(&self, reason: EndReason) {
        *self.reason.lock().expect("evict reason mutex") = Some(reason);
        self.notify.notify_one();
    }

    /// Why this attachment was evicted, or `None` if it was not.
    pub fn reason(&self) -> Option<EndReason> {
        *self.reason.lock().expect("evict reason mutex")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// The bridge learns it was evicted by waking, and announces WHY by reading
    /// the token — so a wake that arrives before the reason does would announce
    /// nothing. `fire` records first and wakes second; this pins that order from
    /// the waiter's side.
    #[tokio::test]
    async fn evict_token_carries_its_reason_before_waking() {
        let token = Arc::new(EvictToken::new());
        assert_eq!(token.reason(), None, "a fresh token names no reason");

        // Observed from a SECOND task, the way the bridge does: it reads the
        // reason at the moment its wait resolves. Asserting after a synchronous
        // `fire` returned would pass even with the order reversed, since both
        // writes are complete by then.
        let waiter = {
            let token = token.clone();
            tokio::spawn(async move {
                token.notify.notified().await;
                token.reason()
            })
        };
        // Let the waiter park before firing; `notify_one` stores a permit either
        // way, so this only makes the intended interleaving the common one.
        tokio::task::yield_now().await;

        token.fire(EndReason::TakenOver);

        let seen = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("the pre-registered waiter must be woken")
            .expect("the waiter task must not panic");
        assert_eq!(
            seen,
            Some(EndReason::TakenOver),
            "a woken waiter must ALREADY see the reason — otherwise the bridge \
             announces `child-exited` for a takeover (the bridge's unwrap_or fallback)"
        );
        assert_eq!(EndReason::TakenOver.as_wire(), "taken-over");
        assert_eq!(EndReason::ChildExited.as_wire(), "child-exited");
        assert_eq!(EndReason::DaemonShutdown.as_wire(), "daemon-shutdown");
    }

    /// The record serialises the checkout only when there is one: an older
    /// peer's listing (and a primary-tree console) keeps its exact shape.
    #[test]
    fn session_info_serialises_checkout_only_when_present() {
        let info = |checkout: Option<&str>| SessionInfo {
            id: 1,
            repo: "owner/ralphy".to_string(),
            agent: "claude".to_string(),
            kind: "agent".to_string(),
            started_at: 1,
            environment: None,
            name: None,
            checkout: checkout.map(str::to_string),
            agent_state: None,
        };
        let primary = serde_json::to_value(info(None)).unwrap();
        assert!(primary.get("checkout").is_none(), "{primary}");
        let linked = serde_json::to_value(info(Some("wt-a"))).unwrap();
        assert_eq!(linked["checkout"], "wt-a");
    }
}
