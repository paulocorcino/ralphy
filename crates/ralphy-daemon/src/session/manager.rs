//! The session table (the tmux model, #166): spawn, attach, watch, list and
//! close the daemon-owned sessions, and pump each child's bytes to its
//! subscribers.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedReceiver;

use super::{EndReason, EvictToken, Session, SessionId, SessionInfo, SessionSpec};

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

/// Per-session scrollback cap. A byte bound (not a line bound) is the simplest
/// structure that satisfies the "chatty session cannot grow memory unboundedly"
/// AC; a byte cap may truncate an escape sequence at the replay seam, which
/// xterm.js resynchronizes on the live stream that follows.
const SCROLLBACK_CAP_BYTES: usize = 256 * 1024;

/// Broadcast capacity (chunks) for the live fan-out. Generous so a briefly slow
/// attach only `Lagged`s (the bridge tolerates a gap) rather than blocking the pump.
const BROADCAST_CAP: usize = 1024;

/// A session the daemon owns (the tmux model): the PTY child plus the machinery
/// that lets a client detach and reattach. `scrollback` is the replay ring; `tx`
/// fans live output out to every attachment; `attached` holds the current single
/// WRITER (`None` when detached), `watchers` the tokens of the read-only clients
/// — any number of them, none holding the writer slot.
struct ManagedSession {
    info: SessionInfo,
    session: Mutex<Session>,
    scrollback: Mutex<VecDeque<u8>>,
    tx: broadcast::Sender<Vec<u8>>,
    attached: Mutex<Option<Writer>>,
    watchers: Mutex<Vec<Arc<EvictToken>>>,
    /// The hooks' files and the tail over the status one (ADR-0059 §5);
    /// `None` for a child without hooks. The tail is polled from the pump's
    /// tick — the session's own task, so it dies with the PTY.
    status: Option<StatusTail>,
    /// The last observed state, `None` until the first hook fires. Interior
    /// mutability because `info` is the immutable identity and this is not.
    agent_state: Mutex<Option<crate::agent_state::Observed>>,
}

/// Who holds the writer slot: the bridge's eviction token, and the holder the
/// client named when it claimed the slot, if any (ADR-0051 §9 amendment
/// 2026-09-22) — the one identity a reattach may reclaim the slot as.
struct Writer {
    token: Arc<EvictToken>,
    holder: Option<String>,
}

struct StatusTail {
    files: crate::agent_state::StatusFiles,
    tail: Mutex<crate::agent_state::Tail>,
}

impl ManagedSession {
    /// The identity as the UI lists it, with the agent state rendered at
    /// `now` (§6 staleness). `now` is a parameter so the rule is testable
    /// through [`SessionManager::list_at`] without waiting 45 minutes.
    fn info_at(&self, now: SystemTime) -> SessionInfo {
        let mut info = self.info.clone();
        info.agent_state = self
            .agent_state
            .lock()
            .expect("agent_state mutex")
            .as_ref()
            .map(|o| crate::agent_state::render(o, now));
        info
    }

    /// Read what the hooks appended since the last tick: keep the newest
    /// transition, and on ANY folded line refresh the observation's `seen` —
    /// a `working` that keeps producing `PreToolUse` lines is alive, and the
    /// staleness clock must not age it into `unknown` (§6). Cheap when
    /// nothing changed (an open, a seek, an empty read).
    fn poll_status(&self) {
        let Some(status) = &self.status else {
            return;
        };
        let polled = status.tail.lock().expect("tail mutex").poll();
        let mut slot = self.agent_state.lock().expect("agent_state mutex");
        if let Some(last) = polled.transitions.into_iter().last() {
            *slot = Some(last);
        } else if polled.activity {
            if let Some(obs) = slot.as_mut() {
                obs.seen = SystemTime::now();
            }
        }
    }

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
        environment: Option<String>,
        checkout: Option<String>,
        spec: SessionSpec,
    ) -> Result<(SessionId, Attachment)> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.spawn_attached_as(id, repo, agent, kind, environment, checkout, spec)
    }

    /// Reserve the id the NEXT [`spawn_attached_as`](Self::spawn_attached_as)
    /// will use — the agent-state files are named by it and must exist before
    /// the child that reads them is launched (ADR-0059 §5).
    pub fn reserve_id(&self) -> SessionId {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// [`spawn_attached`](Self::spawn_attached) with an id from
    /// [`reserve_id`](Self::reserve_id).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_attached_as(
        self: &Arc<Self>,
        id: SessionId,
        repo: String,
        agent: String,
        kind: String,
        environment: Option<String>,
        checkout: Option<String>,
        spec: SessionSpec,
    ) -> Result<(SessionId, Attachment)> {
        // Lifted before the spec is consumed by the spawn.
        let name = spec.name.clone();
        let status = spec.status.clone().map(|files| StatusTail {
            tail: Mutex::new(crate::agent_state::Tail::new(files.status.clone())),
            files,
        });
        // The hook files were written before the spawn (the child reads them
        // on start); a spawn that fails leaves nothing to tail and nothing
        // that would remove them at session end, so they go here.
        let mut session = match Session::spawn(spec) {
            Ok(session) => session,
            Err(e) => {
                if let Some(status) = &status {
                    status.files.remove();
                }
                return Err(e);
            }
        };
        let output = session.take_output();
        let (tx, _rx) = broadcast::channel(BROADCAST_CAP);
        let info = SessionInfo {
            id,
            repo,
            agent,
            kind,
            environment,
            name,
            checkout,
            agent_state: None,
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
            watchers: Mutex::new(Vec::new()),
            status,
            agent_state: Mutex::new(None),
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
        self.attach_as(id, takeover, None)
    }

    /// [`attach`](Self::attach) as `holder`. A busy slot whose writer claimed it
    /// as the SAME holder is reclaimed without `takeover`: the incumbent is this
    /// client's own earlier socket, which a link change left half-open behind a
    /// tunnel (ADR-0051 §9 amendment 2026-09-22). A slot held by anyone else —
    /// or by a writer that named no holder — stays `Busy`.
    pub fn attach_as(
        self: &Arc<Self>,
        id: SessionId,
        takeover: bool,
        holder: Option<&str>,
    ) -> Result<Attachment, AttachError> {
        let token = Arc::new(EvictToken::new());
        let sess = {
            // REGISTRATION INVARIANT: the `sessions` lock is held ACROSS the slot
            // claim, and every end (`close`, the pump's EOF) evicts while holding
            // that same lock. Otherwise an end slipping between "this id is live"
            // and "this token is registered" leaves a token nobody ever fires —
            // and that bridge parks forever: `notified` never completes, and
            // `rx.recv()` cannot return `Closed` because the `Attachment` itself
            // keeps `tx` alive, so the browser shows a dead console as live.
            let map = self.sessions.lock().expect("sessions mutex");
            let sess = map.get(&id).cloned().ok_or(AttachError::Unknown)?;
            let mut slot = sess.attached.lock().expect("attached mutex");
            if let Some(existing) = slot.as_ref() {
                let own = holder.is_some() && existing.holder.as_deref() == holder;
                if !takeover && !own {
                    return Err(AttachError::Busy);
                }
                // Break the incumbent's bridge loop; its guard-drop will NOT clear
                // this new token (ptr_eq mismatch). `notify_one` (not
                // `notify_waiters`) because each token has exactly ONE waiter and
                // `notify_one` STORES a permit if the incumbent is momentarily not
                // parked (mid-iteration), so the eviction can never be lost.
                existing.token.fire(EndReason::TakenOver);
            }
            *slot = Some(Writer {
                token: token.clone(),
                holder: holder.map(str::to_owned),
            });
            drop(slot);
            sess
        };
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
            writer: true,
            _guard: AttachGuard {
                sess: sess.clone(),
                token,
                writer: true,
            },
            sess,
        })
    }

    /// Attach to an existing session as a READ-ONLY watcher (issue #334): the
    /// same replay and the same live stream, but the writer slot is untouched, so
    /// this NEVER refuses with `Busy` and never evicts anyone. A client is a
    /// watcher by construction — it asked to be one; there is no spectator mode
    /// and no second route.
    ///
    /// EXACTLY-ONCE REPLAY INVARIANT: snapshot+`subscribe()` under the scrollback
    /// lock, exactly as [`attach`] does — the invariant holds for a watcher too.
    ///
    /// [`attach`]: SessionManager::attach
    pub fn watch(self: &Arc<Self>, id: SessionId) -> Result<Attachment, AttachError> {
        let token = Arc::new(EvictToken::new());
        let sess = {
            // Same REGISTRATION INVARIANT as `attach` — see its comment.
            let map = self.sessions.lock().expect("sessions mutex");
            let sess = map.get(&id).cloned().ok_or(AttachError::Unknown)?;
            sess.watchers
                .lock()
                .expect("watchers mutex")
                .push(token.clone());
            sess
        };
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
            writer: false,
            _guard: AttachGuard {
                sess: sess.clone(),
                token,
                writer: false,
            },
            sess,
        })
    }

    /// The live sessions, ordered by id (the `BTreeMap` key order).
    pub fn list(&self) -> Vec<SessionInfo> {
        self.list_at(SystemTime::now())
    }

    /// [`list`](Self::list) with the staleness clock supplied (ADR-0059 §6):
    /// what `/api/sessions` would say if it were asked at `now`.
    pub fn list_at(&self, now: SystemTime) -> Vec<SessionInfo> {
        self.sessions
            .lock()
            .expect("sessions mutex")
            .values()
            .map(|s| s.info_at(now))
            .collect()
    }

    /// Whether a live session of `repo` was spawned in the worktree `checkout`
    /// — the daemon's own `worktree.remove` gate (ADR-0063 §2): the CLI cannot
    /// see this table. A free console carries `checkout: None` and never matches.
    pub fn console_in(&self, repo: &str, checkout: &str) -> bool {
        self.sessions
            .lock()
            .expect("sessions mutex")
            .values()
            .any(|s| s.info.repo == repo && s.info.checkout.as_deref() == Some(checkout))
    }

    /// A single session's identity, or `None` if it is not (or no longer) live.
    pub fn get(&self, id: SessionId) -> Option<SessionInfo> {
        self.sessions
            .lock()
            .expect("sessions mutex")
            .get(&id)
            .map(|s| s.info_at(SystemTime::now()))
    }

    /// Close a session: remove it from the map, evict every attached client
    /// (writer AND watchers), and tree-kill the child (the pump then reaches EOF
    /// and self-removes, a no-op). Returns whether the id existed. Idempotent.
    ///
    /// The reason is `ChildExited` rather than a fourth word: this path DOES kill
    /// the child, and the client's vocabulary is fixed at three (issue #334).
    pub fn close(&self, id: SessionId) -> bool {
        let sess = {
            let mut map = self.sessions.lock().expect("sessions mutex");
            let Some(sess) = map.remove(&id) else {
                return false;
            };
            // Evicted UNDER the `sessions` lock — see `attach`'s REGISTRATION
            // INVARIANT: an attachment registered after this point would never be
            // told the session ended.
            evict_all(&sess, EndReason::ChildExited);
            sess
        };
        sess.session.lock().expect("session mutex").close();
        true
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
                    // The agent-state tail rides the same tick (ADR-0059 §5).
                    sess.poll_status();
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
        // The hooks' files go with the session (§6: never persisted).
        if let Some(status) = &sess.status {
            status.files.remove();
        }
        // Child EOF: a session ends besides `close` only when its child exits
        // (issue #166). Remove it from the list, then evict any attached client so
        // its bridge loop ends and the browser sees the session close.
        match manager.upgrade() {
            // Removal and eviction under ONE `sessions` guard — see `attach`'s
            // REGISTRATION INVARIANT.
            Some(manager) => {
                let mut map = manager.sessions.lock().expect("sessions mutex");
                map.remove(&sess.info.id);
                evict_all(&sess, EndReason::ChildExited);
            }
            // The manager is gone, so no new attachment can be registered; the
            // ones already holding this session still have to be told.
            None => evict_all(&sess, EndReason::ChildExited),
        }
    });
}

/// Fire every token attached to this session — the single writer and each
/// watcher — with the same reason. Both lists, because a watcher that is never
/// told the session ended would sit on a dead socket forever.
fn evict_all(sess: &ManagedSession, reason: EndReason) {
    if let Some(writer) = sess.attached.lock().expect("attached mutex").as_ref() {
        writer.token.fire(reason);
    }
    for watcher in sess.watchers.lock().expect("watchers mutex").iter() {
        watcher.fire(reason);
    }
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
/// for the live stream, and an `evict` token the bridge waits on to learn WHY it
/// ended (taken over, child exited, daemon shutting down). `writer` is the role:
/// `true` for the single writer, `false` for a watcher. Dropping it releases the
/// writer slot (or deregisters the watcher) WITHOUT closing the session (the tmux
/// detach).
pub struct Attachment {
    pub snapshot: Vec<u8>,
    pub rx: broadcast::Receiver<Vec<u8>>,
    pub evict: Arc<EvictToken>,
    pub writer: bool,
    _guard: AttachGuard,
    sess: Arc<ManagedSession>,
}

impl Attachment {
    /// Feed a client keystroke to the child. The single-writer policy makes this
    /// race-free with any other browser.
    ///
    /// A WATCHER's input is dropped here and reports `Ok(())` — a deliberate
    /// policy no-op, not a swallowed error: nothing failed, the client simply
    /// does not hold the baton, and returning `Err` would tear its bridge down
    /// for typing into a window it is allowed to keep watching.
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        if !self.writer {
            return Ok(());
        }
        self.sess.write(bytes)
    }

    /// Propagate a client resize to the PTY so the child's TUI reflows. A
    /// watcher's resize is dropped for the same reason as its keystrokes (see
    /// [`write`]) — one PTY has one geometry, and it belongs to the writer.
    ///
    /// [`write`]: Attachment::write
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        if !self.writer {
            return Ok(());
        }
        self.sess.resize(rows, cols)
    }

    /// Record `holder` on the writer slot this attachment holds — how a fresh
    /// launch, which claims its slot inside the spawn, names its holder. A
    /// no-op for a watcher and for a slot another attachment has since taken.
    pub fn hold_as(&self, holder: &str) {
        if !self.writer {
            return;
        }
        let mut slot = self.sess.attached.lock().expect("attached mutex");
        if let Some(writer) = slot.as_mut() {
            if Arc::ptr_eq(&writer.token, &self._guard.token) {
                writer.holder = Some(holder.to_owned());
            }
        }
    }
}

/// Deregisters this attachment on drop. A WRITER clears the single-writer slot —
/// but ONLY when the slot still holds THIS attachment's token, so an evicted
/// incumbent's guard-drop does not clobber the taker's slot (`ptr_eq` mismatch),
/// which is what makes takeover race-free. A WATCHER removes exactly its own
/// token from the watcher list, by the same identity test.
struct AttachGuard {
    sess: Arc<ManagedSession>,
    token: Arc<EvictToken>,
    writer: bool,
}

impl Drop for AttachGuard {
    fn drop(&mut self) {
        if self.writer {
            let mut slot = self.sess.attached.lock().expect("attached mutex");
            if let Some(existing) = slot.as_ref() {
                if Arc::ptr_eq(&existing.token, &self.token) {
                    *slot = None;
                }
            }
        } else {
            self.sess
                .watchers
                .lock()
                .expect("watchers mutex")
                .retain(|w| !Arc::ptr_eq(w, &self.token));
        }
    }
}

#[cfg(test)]
mod tests;
