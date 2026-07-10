//! The workbench session manager (docs/adr/0032 §2): a deep module that turns
//! session verbs — spawn, write, resize, close — into a live PTY child and a
//! byte stream, knowing nothing about the HTTP transport that carries those
//! bytes (the socket bridge lives in `lib.rs`). Keeping it transport-free is
//! what lets it be tested against a helper bin with no socket
//! (`tests/session_roundtrip.rs`) and guarded by `tests/session_transport_free.rs`.
//!
//! The blocking PTY reader is bridged to async the way ADR-0032 prescribes: a
//! `std::thread` drains the master and forwards each chunk over an unbounded
//! channel, so a sync read never blocks the tokio runtime and the send never
//! blocks the reader.

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::thread::JoinHandle;

use anyhow::Result;
use ralphy_pty::{PtyCommand, PtySession};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// A program-neutral description of what to launch inside the PTY. Program +
/// args are already resolved (no agent knowledge), so `Session::spawn` is
/// testable against any helper bin.
pub struct SessionSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub rows: u16,
    pub cols: u16,
}

/// The agents the launcher can start. Maps to a concrete program via
/// [`spec_for`]; the bare interactive launch (no extra args) is what opens each
/// vendor's TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    OpenCode,
}

impl Agent {
    /// Parse the `agent=` query value. Unknown values yield `None` so the route
    /// can reject them rather than launching a surprise program.
    pub fn from_query(value: &str) -> Option<Agent> {
        match value {
            "claude" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            "opencode" => Some(Agent::OpenCode),
            _ => None,
        }
    }

    /// The program name to resolve on `PATH` for this agent.
    fn program_name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::OpenCode => "opencode",
        }
    }
}

/// Environment override pointing the launcher at a stand-in program (the test
/// helper bin). A test-only seam — inert in production — because an integration
/// test's binary is not reachable from a `#[cfg(test)]` path in the compiled lib.
const AGENT_OVERRIDE_ENV: &str = "RALPHY_DAEMON_AGENT_OVERRIDE";

/// Build the launch spec for `agent` in `cwd` at the given terminal size. The
/// program is resolved through `ralphy_proc_util::resolve_program` (Windows
/// `.cmd`/`.exe` shims included), unless `RALPHY_DAEMON_AGENT_OVERRIDE` names a
/// program to run instead.
pub fn spec_for(agent: Agent, cwd: PathBuf, rows: u16, cols: u16) -> SessionSpec {
    let program = match std::env::var_os(AGENT_OVERRIDE_ENV) {
        Some(over) => over,
        None => ralphy_proc_util::resolve_program(agent.program_name()),
    };
    SessionSpec {
        program,
        args: Vec::new(),
        cwd,
        rows,
        cols,
    }
}

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
        let cmd = PtyCommand::new(spec.program)
            .args(spec.args)
            .cwd(&spec.cwd)
            .size(spec.rows, spec.cols);
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
