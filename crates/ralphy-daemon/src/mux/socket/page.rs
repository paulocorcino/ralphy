//! What channel 0 of a page socket sends and takes: the heartbeat, the
//! session changes and pushes, and the page's presence reports.

use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::mux::ChannelSender;
use crate::routes::{build_presence, send_command_on, Push, SESSIONS_DIRTY};
use crate::{identity, presence_board, protocol};

/// The entry of one page socket on the presence board, removed when the
/// socket ends, on every path.
pub(super) struct PageEntry {
    board: presence_board::Shared,
    conn: Option<u64>,
}

impl PageEntry {
    /// A poisoned board is logged, and this socket's reports are not kept.
    pub(super) fn open(board: presence_board::Shared) -> Self {
        let conn = match board.lock() {
            Ok(mut b) => Some(b.open()),
            Err(e) => {
                tracing::warn!(error = %e, "the presence board is poisoned");
                None
            }
        };
        PageEntry { board, conn }
    }

    /// Put the payload of a `presence.report` from the page on the board.
    pub(super) fn report(&self, payload: &serde_json::Value) {
        let Some(conn) = self.conn else {
            return;
        };
        if let Ok(mut board) = self.board.lock() {
            board.report(conn, payload, std::time::SystemTime::now());
        }
    }
}

impl Drop for PageEntry {
    fn drop(&mut self) {
        let Some(conn) = self.conn else {
            return;
        };
        match self.board.lock() {
            Ok(mut board) => board.close(conn),
            Err(e) => tracing::warn!(error = %e, "the presence board is poisoned"),
        }
    }
}

/// One event of the session and push buses.
pub(super) enum Event {
    Sessions(Result<(), RecvError>),
    Push(Result<Push, RecvError>),
}

/// The session and push buses a page socket relays. A receiver that lagged
/// sends every verb once; a closed one turns its arm off, so the loop never
/// spins on it.
pub(super) struct Arms {
    sessions: broadcast::Receiver<()>,
    pushes: broadcast::Receiver<Push>,
    sessions_open: bool,
    pushes_open: bool,
}

/// What one arm tells the loop to do next.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// Wait for the next event.
    Go,
    /// The bus is closed: turn its arm off and go on.
    CloseArm,
    /// A send failed: end the loop.
    Stop,
}

impl Next {
    /// `Go` when the send worked, `Stop` when it failed.
    fn sent(ok: bool) -> Next {
        if ok {
            Next::Go
        } else {
            Next::Stop
        }
    }
}

impl Arms {
    pub(super) fn new(
        sessions: broadcast::Receiver<()>,
        pushes: broadcast::Receiver<Push>,
    ) -> Arms {
        Arms {
            sessions,
            pushes,
            sessions_open: true,
            pushes_open: true,
        }
    }

    /// The next event of a bus that is still open. Cancel safe: nothing is
    /// lost when the wait is dropped. Waits for ever once both are closed.
    pub(super) async fn recv(&mut self) -> Event {
        tokio::select! {
            changed = self.sessions.recv(), if self.sessions_open => Event::Sessions(changed),
            pushed = self.pushes.recv(), if self.pushes_open => Event::Push(pushed),
            else => std::future::pending().await,
        }
    }

    /// Send what `event` says on channel 0. `true` when a send failed and
    /// the socket ends.
    pub(super) async fn send(&mut self, out: &ChannelSender, event: Event) -> bool {
        let (next, open) = match event {
            Event::Sessions(changed) => (on_sessions(out, changed).await, &mut self.sessions_open),
            Event::Push(pushed) => (on_push(out, pushed).await, &mut self.pushes_open),
        };
        match next {
            Next::Go => false,
            Next::CloseArm => {
                *open = false;
                false
            }
            Next::Stop => true,
        }
    }
}

async fn send_push(out: &ChannelSender, verb: &str, payload: serde_json::Value) -> bool {
    send_command_on(out, 0, verb, payload).await.is_ok()
}

/// The sessions arm: a change, or a lag, is one `sessions.dirty`.
async fn on_sessions(out: &ChannelSender, changed: Result<(), RecvError>) -> Next {
    match changed {
        Ok(()) | Err(RecvError::Lagged(_)) => {
            Next::sent(send_push(out, SESSIONS_DIRTY, serde_json::json!({})).await)
        }
        Err(RecvError::Closed) => Next::CloseArm,
    }
}

/// The pushes arm: a push is sent as its verb; a lag sends every verb once.
async fn on_push(out: &ChannelSender, pushed: Result<Push, RecvError>) -> Next {
    match pushed {
        Ok(p) => Next::sent(send_push(out, p.verb(), p.payload()).await),
        Err(RecvError::Lagged(_)) => Next::sent(send_every_push(out).await),
        Err(RecvError::Closed) => Next::CloseArm,
    }
}

/// `sessions.dirty` and every [`Push`] once; no send after the first one that
/// fails. `false` when a send failed.
async fn send_every_push(out: &ChannelSender) -> bool {
    let mut sent = send_push(out, SESSIONS_DIRTY, serde_json::json!({})).await;
    for p in Push::all() {
        sent = sent && send_push(out, p.verb(), p.payload()).await;
    }
    sent
}

/// Send one presence heartbeat. `false` when the send failed.
pub(super) async fn send_presence(
    out: &ChannelSender,
    identity: Option<&identity::Identity>,
    start: Instant,
) -> bool {
    let frame = build_presence(identity, start.elapsed());
    out.send(protocol::encode(&frame)).await.is_ok()
}

/// How often the heartbeat goes out.
pub(super) const HEARTBEAT_EVERY: Duration = Duration::from_secs(2);
