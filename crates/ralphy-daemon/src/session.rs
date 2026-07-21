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

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use ralphy_pty::{PtyCommand, PtySession};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::{broadcast, Notify};

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
    Copilot,
    Cursor,
    Kimi,
    OpenCode,
}

impl Agent {
    /// Every launchable vendor. The anti-drift tests (the workbench trio, the
    /// usage-store resolvers) enumerate the daemon's vendors from here, so a
    /// seventh variant reds them instead of passing silently.
    pub const ALL: [Agent; 6] = [
        Agent::Claude,
        Agent::Codex,
        Agent::Copilot,
        Agent::Cursor,
        Agent::Kimi,
        Agent::OpenCode,
    ];

    /// Parse the `agent=` query value. Unknown values yield `None` so the route
    /// can reject them rather than launching a surprise program.
    pub fn from_query(value: &str) -> Option<Agent> {
        match value {
            "claude" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            "copilot" => Some(Agent::Copilot),
            "cursor" => Some(Agent::Cursor),
            "kimi" => Some(Agent::Kimi),
            "opencode" => Some(Agent::OpenCode),
            _ => None,
        }
    }

    /// The program name to resolve on `PATH` for this agent.
    fn program_name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            // The GitHub Copilot CLI ships as `copilot`, the same name the
            // adapter resolves for its headless calls (ADR-0041).
            Agent::Copilot => "copilot",
            // Cursor ships one binary under two names (`cursor-agent`, `agent`)
            // across three install roots and is on `PATH` under NEITHER
            // (ADR-0042 D14) — so this name is only the fallback that makes a
            // spawn failure legible; [`Agent::resolve_program`] does the real work.
            Agent::Cursor => "cursor-agent",
            // `kimi-code` ships its binary as `kimi` — the same name the adapter
            // resolves for its headless calls (ADR-0028 D5).
            Agent::Kimi => "kimi",
            Agent::OpenCode => "opencode",
        }
    }

    /// What a `Command` should be constructed with. Every vendor but Cursor is
    /// found on `PATH` (plus the `~/.local/bin` fallback); Cursor needs its own
    /// locator, shared with the run path so detection and execution cannot
    /// disagree (ADR-0042 D14/D19).
    fn resolve_program(self) -> OsString {
        match self {
            Agent::Cursor => ralphy_proc_util::cursor::locate_cursor()
                .map(PathBuf::into_os_string)
                .unwrap_or_else(|| self.program_name().into()),
            _ => ralphy_proc_util::resolve_program(self.program_name()),
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
        None => agent.resolve_program(),
    };
    SessionSpec {
        program,
        args: Vec::new(),
        cwd,
        rows,
        cols,
    }
}

/// Whether the operator opted in to Cursor's codebase upload for `repo_root`,
/// read from `<repo_root>/.ralphy/settings.json`'s
/// `["cursor"]["allow_codebase_indexing_i_understand_the_risk"]`.
///
/// The schema is `ralphy-agent-cursor`'s `CursorSettings`, but the daemon may not
/// import the core (ADR-0032 §10), so it reparses the file — the same precedent
/// `registry.rs` sets for `repos.toml`. `the_optin_key_matches_the_adapters_own_schema`
/// reds if the adapter renames either key.
///
/// INVARIANT: every failure path — no file, unreadable, malformed JSON, wrong
/// type — yields `false`. Refusal is the safe default: an unreadable settings
/// file must never be what opens the upload.
pub fn cursor_indexing_allowed(repo_root: &Path) -> bool {
    let path = repo_root.join(".ralphy").join("settings.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("cursor")?
                .get("allow_codebase_indexing_i_understand_the_risk")?
                .as_bool()
        })
        .unwrap_or(false)
}

/// The platform's default interactive shell for the free console (issue #167).
/// Windows prefers `pwsh` (PowerShell 7), falling back to Windows PowerShell,
/// then `%ComSpec%`/`cmd.exe` (both always present, unlike `pwsh`) so the
/// console stays usable on a box without PowerShell 7. Off Windows, the user's
/// login shell (`$SHELL`), falling back to `/bin/sh`.
fn default_shell() -> OsString {
    if cfg!(windows) {
        ralphy_proc_util::locate_program("pwsh")
            .or_else(|| ralphy_proc_util::locate_program("powershell"))
            .map(Into::into)
            .or_else(|| std::env::var_os("ComSpec"))
            .unwrap_or_else(|| "cmd.exe".into())
    } else {
        std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into())
    }
}

/// The free console's working directory: the chosen repo's path, or the home
/// directory when none was chosen, or `.` when even the home directory cannot
/// be resolved (so a console still spawns rather than failing outright).
pub fn console_cwd(repo_path: Option<PathBuf>) -> PathBuf {
    repo_path
        .or_else(ralphy_proc_util::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Build the launch spec for the free console (issue #167): the platform shell
/// in `cwd`, no args — unless `RALPHY_DAEMON_AGENT_OVERRIDE` names a stand-in
/// program instead (the same test seam [`spec_for`] uses).
pub fn console_spec(cwd: PathBuf, rows: u16, cols: u16) -> SessionSpec {
    let program = match std::env::var_os(AGENT_OVERRIDE_ENV) {
        Some(over) => over,
        None => default_shell(),
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

/// Append `bytes` to the scrollback `ring`, then drop from the FRONT until it is
/// no longer over `cap` — a byte-bounded ring so a chatty session cannot grow the
/// daemon's memory without bound (issue #166 AC2). Front-drop is intentional:
/// scrollback keeps the most RECENT output; the truncated seam is resynchronized
/// by the live stream that follows the replay.
fn push_capped(ring: &mut std::collections::VecDeque<u8>, bytes: &[u8], cap: usize) {
    ring.extend(bytes.iter().copied());
    while ring.len() > cap {
        ring.pop_front();
    }
}

/// A daemon-owned session id. `u64` fits the codec's `Frame::Terminal { session }`
/// field directly and is monotonic within a daemon lifetime, so ids never collide.
pub type SessionId = u64;

/// Per-session scrollback cap. A byte bound (not a line bound) is the simplest
/// structure that satisfies the "chatty session cannot grow memory unboundedly"
/// AC; a byte cap may truncate an escape sequence at the replay seam, which
/// xterm.js resynchronizes on the live stream that follows.
const SCROLLBACK_CAP_BYTES: usize = 256 * 1024;

/// Broadcast capacity (chunks) for the live fan-out. Generous so a briefly slow
/// attach only `Lagged`s (the bridge tolerates a gap) rather than blocking the pump.
const BROADCAST_CAP: usize = 1024;

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
}

/// A session the daemon owns (the tmux model): the PTY child plus the machinery
/// that lets a client detach and reattach. `scrollback` is the replay ring; `tx`
/// fans live output out to the current attachment; `attached` holds the current
/// single writer's eviction token (`None` when detached).
struct ManagedSession {
    info: SessionInfo,
    session: Mutex<Session>,
    scrollback: Mutex<VecDeque<u8>>,
    tx: broadcast::Sender<Vec<u8>>,
    attached: Mutex<Option<Arc<Notify>>>,
}

impl ManagedSession {
    /// Feed raw bytes to the child as terminal input. Behind the session mutex so
    /// the single writer and a concurrent `close` do not race the PTY handle.
    fn write(&self, bytes: &[u8]) -> Result<()> {
        self.session.lock().expect("session mutex").write(bytes)
    }

    /// Resize the PTY window. Behind the session mutex (see [`write`]).
    fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.session
            .lock()
            .expect("session mutex")
            .resize(rows, cols)
    }
}

/// The daemon's set of live sessions (docs/adr/0032 §2, issue #166). Sessions
/// belong to the manager, not to any connection: a client disconnect detaches, never
/// closes. Constructed once inside `router()`; a `Weak` clone is handed to each
/// pump so a finished child can remove itself.
pub struct SessionManager {
    sessions: Mutex<BTreeMap<SessionId, Arc<ManagedSession>>>,
    next_id: AtomicU64,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(BTreeMap::new()),
            // Start at 1: id 0 reads as "unset" and the codec's default session.
            next_id: AtomicU64::new(1),
        }
    }

    /// Spawn a fresh session, start its output pump, and attach to it. The caller
    /// (the WS upgrade) gets the id (for the list/close endpoints and the codec's
    /// `session` field) and an [`Attachment`] to bridge onto the socket. A fresh
    /// session is never busy, so the initial attach always succeeds.
    pub fn spawn_attached(
        self: &Arc<Self>,
        repo: String,
        agent: String,
        kind: String,
        spec: SessionSpec,
    ) -> Result<(SessionId, Attachment)> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut session = Session::spawn(spec)?;
        let output = session.take_output();
        let (tx, _rx) = broadcast::channel(BROADCAST_CAP);
        let info = SessionInfo {
            id,
            repo,
            agent,
            kind,
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        let managed = Arc::new(ManagedSession {
            info,
            session: Mutex::new(session),
            scrollback: Mutex::new(VecDeque::new()),
            tx,
            attached: Mutex::new(None),
        });
        self.sessions
            .lock()
            .expect("sessions mutex")
            .insert(id, managed.clone());
        start_pump(managed.clone(), Arc::downgrade(self), output);
        let attachment = self
            .attach(id, true)
            .map_err(|_| anyhow::anyhow!("fresh session unexpectedly busy"))?;
        Ok((id, attachment))
    }

    /// Attach to an existing session as its single writer, returning a replay
    /// snapshot plus a live receiver. Refuses a busy session unless `takeover`,
    /// in which case the incumbent is evicted first.
    ///
    /// EXACTLY-ONCE REPLAY INVARIANT: `subscribe()` happens UNDER the scrollback
    /// lock, mirroring the pump which holds that same lock across push+send. So
    /// every byte lands in exactly one of {replayed snapshot, live broadcast} —
    /// no gap and no duplicate at the attach seam.
    pub fn attach(
        self: &Arc<Self>,
        id: SessionId,
        takeover: bool,
    ) -> Result<Attachment, AttachError> {
        let sess = {
            let map = self.sessions.lock().expect("sessions mutex");
            map.get(&id).cloned().ok_or(AttachError::Unknown)?
        };
        let token = Arc::new(Notify::new());
        {
            let mut slot = sess.attached.lock().expect("attached mutex");
            if let Some(existing) = slot.as_ref() {
                if !takeover {
                    return Err(AttachError::Busy);
                }
                // Break the incumbent's bridge loop; its guard-drop will NOT clear
                // this new token (ptr_eq mismatch). `notify_one` (not
                // `notify_waiters`) because each token has exactly ONE waiter and
                // `notify_one` STORES a permit if the incumbent is momentarily not
                // parked (mid-iteration), so the eviction can never be lost.
                existing.notify_one();
            }
            *slot = Some(token.clone());
        }
        let (snapshot, rx) = {
            let ring = sess.scrollback.lock().expect("scrollback mutex");
            let snapshot: Vec<u8> = ring.iter().copied().collect();
            let rx = sess.tx.subscribe();
            (snapshot, rx)
        };
        Ok(Attachment {
            snapshot,
            rx,
            evict: token.clone(),
            _guard: AttachGuard {
                sess: sess.clone(),
                token,
            },
            sess,
        })
    }

    /// The live sessions, ordered by id (the `BTreeMap` key order).
    pub fn list(&self) -> Vec<SessionInfo> {
        self.sessions
            .lock()
            .expect("sessions mutex")
            .values()
            .map(|s| s.info.clone())
            .collect()
    }

    /// A single session's identity, or `None` if it is not (or no longer) live.
    pub fn get(&self, id: SessionId) -> Option<SessionInfo> {
        self.sessions
            .lock()
            .expect("sessions mutex")
            .get(&id)
            .map(|s| s.info.clone())
    }

    /// Close a session: remove it from the map, evict any attached client, and
    /// tree-kill the child (the pump then reaches EOF and self-removes, a no-op).
    /// Returns whether the id existed. Idempotent.
    pub fn close(&self, id: SessionId) -> bool {
        let sess = self.sessions.lock().expect("sessions mutex").remove(&id);
        match sess {
            Some(sess) => {
                if let Some(tok) = sess.attached.lock().expect("attached mutex").as_ref() {
                    tok.notify_one();
                }
                sess.session.lock().expect("session mutex").close();
                true
            }
            None => false,
        }
    }
}

/// The output pump for one session: drain the child's output, feed the scrollback
/// ring AND the live broadcast, and remove the session on child EOF. Spawned on
/// the tokio runtime; the `Weak` avoids a manager↔pump reference cycle.
///
/// EXACTLY-ONCE REPLAY INVARIANT: the scrollback lock is held across BOTH the ring
/// push AND `tx.send`, mirrored by [`SessionManager::attach`] holding it across
/// snapshot+subscribe — so an attach never sees a byte both in its snapshot and
/// on its live receiver, nor misses one in the gap.
fn start_pump(
    sess: Arc<ManagedSession>,
    manager: Weak<SessionManager>,
    mut output: UnboundedReceiver<Vec<u8>>,
) {
    tokio::spawn(async move {
        // Poll for a self-exited child alongside draining output: ConPTY does not
        // EOF the reader on child death (only on master drop), so without this a
        // child that exits on its own (e.g. `quit`) would leak its session forever
        // on Windows. On exit, `close()` drops the master → the reader EOFs → the
        // loop below ends. On Unix the reader EOFs directly and the tick is moot.
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                chunk = output.recv() => match chunk {
                    Some(chunk) => {
                        let mut ring = sess.scrollback.lock().expect("scrollback mutex");
                        push_capped(&mut ring, &chunk, SCROLLBACK_CAP_BYTES);
                        let _ = sess.tx.send(chunk);
                        drop(ring);
                    }
                    None => break, // reader EOF (child exited + master dropped)
                },
                _ = tick.tick() => {
                    // One lock spanning the check + close so a client write/resize
                    // cannot interleave between them.
                    let mut session = sess.session.lock().expect("session mutex");
                    if session.has_exited() {
                        // Drops the master → the reader drains then EOFs → break.
                        session.close();
                    }
                }
            }
        }
        // Child EOF: a session ends besides `close` only when its child exits
        // (issue #166). Remove it from the list, then evict any attached client so
        // its bridge loop ends and the browser sees the session close.
        if let Some(manager) = manager.upgrade() {
            manager
                .sessions
                .lock()
                .expect("sessions mutex")
                .remove(&sess.info.id);
        }
        if let Some(tok) = sess.attached.lock().expect("attached mutex").as_ref() {
            tok.notify_one();
        }
    });
}

/// Why an [`attach`] failed. `Unknown` → the id names no live session (`404`);
/// `Busy` → a single writer is already attached and `takeover` was not set (`409`).
///
/// [`attach`]: SessionManager::attach
#[derive(Debug)]
pub enum AttachError {
    Unknown,
    Busy,
}

/// A live attachment to a session: the replay `snapshot` to send first, a `rx`
/// for the live stream, and an `evict` token the bridge waits on to learn it was
/// taken over (or that the child exited). Dropping it releases the single-writer
/// slot WITHOUT closing the session (the tmux detach).
pub struct Attachment {
    pub snapshot: Vec<u8>,
    pub rx: broadcast::Receiver<Vec<u8>>,
    pub evict: Arc<Notify>,
    _guard: AttachGuard,
    sess: Arc<ManagedSession>,
}

impl Attachment {
    /// Feed a client keystroke to the child. The single-writer policy makes this
    /// race-free with any other browser.
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        self.sess.write(bytes)
    }

    /// Propagate a client resize to the PTY so the child's TUI reflows.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.sess.resize(rows, cols)
    }
}

/// Clears the session's single-writer slot on drop — but ONLY when the slot still
/// holds THIS attachment's token. An evicted incumbent's guard-drop therefore does
/// not clobber the taker's slot (`ptr_eq` mismatch), which is what makes takeover
/// race-free.
struct AttachGuard {
    sess: Arc<ManagedSession>,
    token: Arc<Notify>,
}

impl Drop for AttachGuard {
    fn drop(&mut self) {
        let mut slot = self.sess.attached.lock().expect("attached mutex");
        if let Some(existing) = slot.as_ref() {
            if Arc::ptr_eq(existing, &self.token) {
                *slot = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollback_ring_is_bounded() {
        let mut ring = std::collections::VecDeque::new();
        // Fed in ≥2 chunks: proves the cap holds across successive pushes, not
        // only on a single oversized write.
        push_capped(&mut ring, b"012345", 8);
        push_capped(&mut ring, b"6789AB", 8);
        assert_eq!(ring.len(), 8, "ring must not exceed the cap");
        assert_eq!(
            ring.iter().copied().collect::<Vec<u8>>(),
            b"456789AB".to_vec(),
            "the FRONT is dropped and the tail retained"
        );
    }

    #[test]
    fn console_cwd_prefers_chosen_repo_then_falls_back_to_home() {
        assert_eq!(
            console_cwd(Some(PathBuf::from("x"))),
            PathBuf::from("x"),
            "a chosen repo path wins over the home-dir default"
        );
        assert_eq!(
            console_cwd(None),
            ralphy_proc_util::home_dir().unwrap_or_else(|| PathBuf::from(".")),
            "no chosen repo falls back to the home directory (or '.' if unresolvable)"
        );
    }

    #[test]
    fn every_agent_parses_from_its_query_value_and_names_a_program() {
        // The daemon's enum is hand-kept in step with the CLI's `--agent` values
        // (ADR-0040 Tier 4). A vendor missing here is invisible to the compiler —
        // `from_query` just returns `None` and the daemon refuses the spawn, which
        // is exactly how Kimi went unreachable from the workbench (issue #228).
        for (value, agent, program) in [
            ("claude", Agent::Claude, "claude"),
            ("codex", Agent::Codex, "codex"),
            ("copilot", Agent::Copilot, "copilot"),
            ("cursor", Agent::Cursor, "cursor-agent"),
            ("kimi", Agent::Kimi, "kimi"),
            ("opencode", Agent::OpenCode, "opencode"),
        ] {
            assert_eq!(
                Agent::from_query(value),
                Some(agent),
                "`agent={value}` must parse — an unparsed vendor cannot be launched"
            );
            assert_eq!(
                agent.program_name(),
                program,
                "{value} must resolve the program the adapter itself shells"
            );
        }
        assert_eq!(
            Agent::from_query("bash"),
            None,
            "an unknown value stays unparsed rather than launching a surprise program"
        );
    }

    /// Serializes the tests below, which mutate process-global env vars.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// ADR-0042 D14: the Cursor CLI is on `PATH` under NEITHER of its two names, so
    /// `resolve_program`'s PATH search would fall back to the bare `cursor-agent`
    /// and the spawn would fail on a machine where Cursor IS installed. The daemon
    /// must go through the vendor locator instead.
    #[test]
    fn cursor_resolves_off_path_through_the_vendor_locator() {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let restore = [
            ("PATH", std::env::var_os("PATH")),
            ("HOME", std::env::var_os("HOME")),
            ("USERPROFILE", std::env::var_os("USERPROFILE")),
            ("LOCALAPPDATA", std::env::var_os("LOCALAPPDATA")),
            (AGENT_OVERRIDE_ENV, std::env::var_os(AGENT_OVERRIDE_ENV)),
        ];

        let home = tempfile::tempdir().unwrap();
        let empty_path = tempfile::tempdir().unwrap();
        // The cfg-free install shape (`~/.cursor/bin/cursor-agent`, no extension) —
        // the one branch of the locator that matches identically on both platforms.
        let seeded = home.path().join(".cursor").join("bin").join("cursor-agent");
        std::fs::create_dir_all(seeded.parent().unwrap()).unwrap();
        std::fs::write(&seeded, "").unwrap();

        std::env::set_var("PATH", empty_path.path());
        std::env::set_var("HOME", home.path());
        std::env::set_var("USERPROFILE", home.path());
        std::env::remove_var("LOCALAPPDATA");
        std::env::remove_var(AGENT_OVERRIDE_ENV);

        let spec = spec_for(Agent::Cursor, PathBuf::from("."), 24, 80);
        assert_eq!(
            spec.program,
            seeded.clone().into_os_string(),
            "must resolve the installed binary, not the bare `cursor-agent` name"
        );

        // The test seam still wins for Cursor — it must not be bypassed for the one
        // vendor with its own locator, or every integration test loses its stand-in.
        std::env::set_var(AGENT_OVERRIDE_ENV, "stand-in");
        assert_eq!(
            spec_for(Agent::Cursor, PathBuf::from("."), 24, 80).program,
            OsString::from("stand-in")
        );

        for (name, value) in restore {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
        drop(guard);
    }

    /// The daemon reparses the adapter's settings file rather than importing the
    /// core (ADR-0032 §10), so the two schemas can drift silently. This is the pin:
    /// rename either name in the adapter and the daemon's gate reds here.
    #[test]
    fn the_optin_key_matches_the_adapters_own_schema() {
        let src = include_str!("../../ralphy-agent-cursor/src/settings.rs");
        assert!(
            src.contains("allow_codebase_indexing_i_understand_the_risk"),
            "the adapter renamed the opt-in key the daemon reparses"
        );
        assert!(
            src.contains(r#"SECTION: &'static str = "cursor""#),
            "the adapter renamed the settings section the daemon reparses"
        );
    }

    /// The refusal is the safe default: only an explicit `true` opens the upload.
    #[test]
    fn cursor_indexing_allowed_defaults_to_false() {
        let d = tempfile::tempdir().unwrap();
        assert!(
            !cursor_indexing_allowed(d.path()),
            "no .ralphy/ at all must not opt in"
        );

        let settings = d.path().join(".ralphy").join("settings.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        for (body, want) in [
            ("{}", false),
            (
                r#"{"cursor":{"allow_codebase_indexing_i_understand_the_risk":true}}"#,
                true,
            ),
            (
                r#"{"cursor":{"allow_codebase_indexing_i_understand_the_risk":"yes"}}"#,
                false,
            ),
            (
                r#"{"cursor":{"allow_codebase_indexing_i_understand_the_risk":false}}"#,
                false,
            ),
            ("not json", false),
        ] {
            std::fs::write(&settings, body).unwrap();
            assert_eq!(
                cursor_indexing_allowed(d.path()),
                want,
                "settings.json = {body}"
            );
        }
    }
}
