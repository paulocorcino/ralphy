//! The session table (the tmux model, #166): spawn, attach, watch, list and
//! close the daemon-owned sessions, and pump each child's bytes to its
//! subscribers.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tokio::sync::broadcast;

use super::screen;
use super::{EndReason, EvictToken, Session, SessionId, SessionInfo, SessionSpec};

mod agent_status;
mod attachment;
mod id_seq;
mod kept;
mod pump;
mod record;
use agent_status::AgentStatus;
pub use agent_status::TurnEnd;
pub use attachment::{AttachError, Attachment};
use id_seq::IdSeq;
#[cfg(test)]
use kept::publish;
use kept::{replay_and_subscribe, KeptOutput, Subscribed};
use pump::start_pump;
pub use record::RecordClaim;

/// Broadcast capacity (chunks) of the live fan-out: a slow attach lags, never blocks the pump.
const BROADCAST_CAP: usize = 1024;

/// A session the daemon owns (the tmux model): the PTY child plus the machinery
/// that lets a client detach and reattach. `kept` is the ring and the screen; `tx`
/// fans live output out to every attachment; `attached` is the single writer
/// (`None` when detached), `watchers` the read-only clients.
struct ManagedSession {
    info: SessionInfo,
    session: Mutex<Session>,
    kept: Mutex<KeptOutput>,
    tx: broadcast::Sender<Vec<u8>>,
    attached: Mutex<Option<Writer>>,
    watchers: Mutex<Vec<Arc<EvictToken>>>,
    /// The hooks' files and the reader of the status one (ADR-0059 §5);
    /// `None` for a child without hooks. The pump's tick reads it — the
    /// session's own task, so it dies with the PTY.
    status: Option<StatusTail>,
    /// The agent state, the silence rule's clocks and the turn count, behind
    /// one lock. Read the status lines before taking it (see
    /// [`AgentStatus`]).
    agent_status: Mutex<AgentStatus>,
}

/// Who holds the writer slot: the bridge's eviction token, and the holder the
/// client named when it claimed the slot, if any (ADR-0051 §9 amendment
/// 2026-09-22) — the one identity a reattach may reclaim the slot as — with
/// the attempt number of that claim (ADR-0079 D7).
struct Writer {
    token: Arc<EvictToken>,
    holder: Option<String>,
    attempt: Option<u64>,
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
            .agent_status
            .lock()
            .expect("agent_status mutex")
            .shown_at(now);
        info
    }

    /// Feed raw bytes to the child as terminal input. The session lock is held
    /// only to queue the bytes. The operator's input starts the silence clock
    /// (a reply the terminal sent by itself does not) and ends the permission
    /// `waiting` of a vendor that sends no grant event (ADR-0085 D7.5).
    fn write(&self, bytes: &[u8]) -> Result<()> {
        if !bytes.is_empty() && !crate::agent_state::is_terminal_report(bytes) {
            self.agent_status
                .lock()
                .expect("agent_status mutex")
                .on_input(SystemTime::now());
        }
        self.session.lock().expect("session mutex").write(bytes)
    }

    /// Resize the PTY window and the screen to the same clamped size
    /// (ADR-0079 D3). The screen is resized only when the PTY accepted it.
    ///
    /// LOCK ORDER: `kept`, then `session`. This is the one place that holds
    /// both; every other path takes one alone, so the order cannot deadlock.
    /// `kept` is held across both resizes, so output at the new size (ConPTY
    /// repaints at once) reaches the screen after its resize, and two racing
    /// resizes leave the PTY and the screen at the same size.
    fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        let (rows, cols) = screen::clamp_size(rows, cols);
        let mut kept = self.kept.lock().expect("kept output mutex");
        self.session
            .lock()
            .expect("session mutex")
            .resize(rows, cols)?;
        kept.resize(rows, cols);
        drop(kept);
        Ok(())
    }
}

/// The daemon's set of live sessions (docs/adr/0032 §2, issue #166). Sessions
/// belong to the manager, not to any connection: a disconnect detaches, never
/// closes. Each pump holds a `Weak` clone so a finished child removes itself.
pub struct SessionManager {
    sessions: Mutex<BTreeMap<SessionId, Arc<ManagedSession>>>,
    ids: Mutex<IdSeq>,
    /// One message each time the list changes: a session starts or ends, or
    /// its agent state changes. The presence socket relays it as
    /// `sessions.dirty` (ADR-0070 D2 event 1).
    changes: broadcast::Sender<()>,
    /// One gate per window record a launch is running for (ADR-0050 amendment
    /// 2026-10-04). An entry lives while a launch holds or waits on it.
    records: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// Capacity of [`SessionManager::subscribe_changes`]. A receiver that lags
/// reads the list again once, so a small buffer loses nothing.
const CHANGES_CAP: usize = 32;

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    /// A manager whose ids start at 1 and are not recorded anywhere.
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(BTreeMap::new()),
            ids: Mutex::new(IdSeq {
                last: 0,
                file: None,
            }),
            changes: broadcast::channel(CHANGES_CAP).0,
            records: Mutex::new(HashMap::new()),
        }
    }

    /// A manager whose ids continue past the last one recorded in `file`, and
    /// record each new one there (see `IdSeq`).
    pub fn continuing(file: PathBuf) -> Self {
        Self {
            sessions: Mutex::new(BTreeMap::new()),
            ids: Mutex::new(IdSeq::continuing(file)),
            changes: broadcast::channel(CHANGES_CAP).0,
            records: Mutex::new(HashMap::new()),
        }
    }

    /// A receiver that gets one message each time the session list, or a
    /// session's agent state, changes.
    pub fn subscribe_changes(&self) -> broadcast::Receiver<()> {
        self.changes.subscribe()
    }

    fn issue_id(&self) -> SessionId {
        self.ids.lock().expect("session id mutex").next()
    }

    /// Spawn a fresh session, start its output pump, and attach to it. The caller
    /// gets the id and an [`Attachment`] to bridge onto the socket. A fresh
    /// session is never busy. `record` is the window record the launch named;
    /// its [`RecordClaim`] is the caller's.
    #[expect(
        clippy::too_many_arguments,
        reason = "the session's labels, its record and its spec; a public method"
    )]
    pub fn spawn_attached(
        self: &Arc<Self>,
        repo: String,
        agent: String,
        kind: String,
        environment: Option<String>,
        checkout: Option<String>,
        record: Option<String>,
        spec: SessionSpec,
    ) -> Result<(SessionId, Attachment)> {
        let id = self.issue_id();
        self.spawn_attached_as(id, repo, agent, kind, environment, checkout, record, spec)
    }

    /// Reserve the id the NEXT [`spawn_attached_as`](Self::spawn_attached_as)
    /// will use — the agent-state files are named by it and must exist before
    /// the child that reads them is launched (ADR-0059 §5).
    pub fn reserve_id(&self) -> SessionId {
        self.issue_id()
    }

    /// [`spawn_attached`](Self::spawn_attached) with an id from
    /// [`reserve_id`](Self::reserve_id).
    #[expect(
        clippy::too_many_arguments,
        reason = "the session's id, labels, record and spec; a public method"
    )]
    pub fn spawn_attached_as(
        self: &Arc<Self>,
        id: SessionId,
        repo: String,
        agent: String,
        kind: String,
        environment: Option<String>,
        checkout: Option<String>,
        record: Option<String>,
        mut spec: SessionSpec,
    ) -> Result<(SessionId, Attachment)> {
        // The PTY and the screen start at the same clamped size (ADR-0079 D3).
        (spec.rows, spec.cols) = screen::clamp_size(spec.rows, spec.cols);
        let kept = KeptOutput::new(id, spec.rows, spec.cols);
        // Lifted before the spec is consumed by the spawn.
        let name = spec.name.clone();
        let unreported = spec
            .unreported_reason
            .take()
            .map(|reason| (reason, SystemTime::now()));
        let agent_status = AgentStatus::new(id, spec.status.as_ref().map(|f| f.vendor), unreported);
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
            record,
            agent_state: None,
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        let managed = Arc::new(ManagedSession {
            info,
            session: Mutex::new(session),
            kept: Mutex::new(kept),
            tx,
            attached: Mutex::new(None),
            watchers: Mutex::new(Vec::new()),
            status,
            agent_status: Mutex::new(agent_status),
        });
        self.sessions
            .lock()
            .expect("sessions mutex")
            .insert(id, managed.clone());
        notify(&self.changes);
        start_pump(
            managed.clone(),
            Arc::downgrade(self),
            output,
            self.changes.clone(),
        );
        let attachment = self
            .attach(id, true)
            .map_err(|error| anyhow::anyhow!("fresh session unexpectedly busy: {error:?}"))?;
        Ok((id, attachment))
    }

    /// Attach to an existing session as its single writer, returning a replay
    /// (the screen snapshot) plus a live receiver. Refuses a busy session unless
    /// `takeover`, in which case the incumbent is evicted first.
    ///
    /// EXACTLY-ONCE REPLAY INVARIANT: the replay and `subscribe()` are taken
    /// UNDER the `kept` lock (`replay_and_subscribe`), mirroring the pump,
    /// which holds that same lock across keep+send (`kept::publish`). So every byte
    /// lands in exactly one of {replay, live broadcast} — no gap and no
    /// duplicate at the attach seam.
    pub fn attach(
        self: &Arc<Self>,
        id: SessionId,
        takeover: bool,
    ) -> Result<Attachment, AttachError> {
        self.attach_as(id, takeover, None, None, None)
    }

    /// [`attach`](Self::attach) as `holder`. A busy slot whose writer claimed it
    /// as the SAME holder is reclaimed without `takeover`: the incumbent is this
    /// client's own earlier socket, which a link change left half-open behind a
    /// tunnel (ADR-0051 §9 amendment 2026-09-22). A slot held by anyone else —
    /// or by a writer that named no holder — stays `Busy`.
    ///
    /// `attempt` orders the claims of one holder (ADR-0079 D7): a claim as the
    /// slot's own holder with a LOWER attempt than the writer's and no
    /// `takeover` is a late socket the client already replaced, so it is
    /// `Superseded` and evicts nobody. A `takeover` is the operator's own
    /// choice and always wins; a page reconnect never sends one. When either
    /// side has no attempt, the holder rule above decides alone.
    ///
    /// `since` is the byte count the client's terminal already shows: the
    /// replay is then only the bytes after it, when the ring still has them
    /// all (ADR-0079 D8). See [`Attachment::resume`].
    pub fn attach_as(
        self: &Arc<Self>,
        id: SessionId,
        takeover: bool,
        holder: Option<&str>,
        attempt: Option<u64>,
        since: Option<u64>,
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
                if !takeover
                    && own
                    && matches!(
                        (attempt, existing.attempt),
                        (Some(claim), Some(held)) if claim < held
                    )
                {
                    return Err(AttachError::Superseded);
                }
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
                attempt,
            });
            drop(slot);
            sess
        };
        Ok(Attachment::new(
            replay_and_subscribe(&sess.kept, &sess.tx, since),
            token,
            true,
            sess,
        ))
    }

    /// Attach to an existing session as a READ-ONLY watcher (issue #334): the
    /// same replay and live stream, but the writer slot is untouched, so this
    /// never refuses with `Busy` and never evicts anyone.
    ///
    /// The EXACTLY-ONCE REPLAY INVARIANT of [`attach`] holds for a watcher too.
    /// `since` is read as [`attach_as`] reads it.
    ///
    /// [`attach`]: SessionManager::attach
    /// [`attach_as`]: SessionManager::attach_as
    pub fn watch(
        self: &Arc<Self>,
        id: SessionId,
        since: Option<u64>,
    ) -> Result<Attachment, AttachError> {
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
        Ok(Attachment::new(
            replay_and_subscribe(&sess.kept, &sess.tx, since),
            token,
            false,
            sess,
        ))
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
    /// The reason is `ChildExited`: the client's vocabulary is fixed at three
    /// (issue #334).
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
        notify(&self.changes);
        true
    }
}

/// Tell the subscribers the session list changed. With no subscriber there is
/// nothing to tell: the next tab that opens reads the list anyway.
fn notify(changes: &broadcast::Sender<()>) {
    if changes.send(()).is_err() {
        // No subscriber: nothing to tell.
    }
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

#[cfg(test)]
mod tests;
