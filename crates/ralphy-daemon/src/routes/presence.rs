//! `/ws`: the presence heartbeat.

use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use tokio::sync::broadcast;

use crate::protocol::{Command, Frame, Presence};
use crate::{identity, protocol};

/// A change to a shown fact the daemon owns, relayed on every open `/ws` as a
/// `<fact>.dirty` command with id 0 (ADR-0070 D2 event 1). `/ws` carries them
/// because it is the one socket every tab keeps open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Push {
    /// `PUT /api/desk` changed the desk; `tab` is the writer's tab id, so
    /// that tab does not read its own write again.
    Desk { tab: Option<String> },
}

impl Push {
    fn verb(&self) -> &'static str {
        match self {
            Push::Desk { .. } => "desk.dirty",
        }
    }

    fn payload(&self) -> serde_json::Value {
        match self {
            Push::Desk { tab } => serde_json::json!({ "tab": tab }),
        }
    }

    /// Every push once: what a receiver that lagged sends, because a read
    /// again is harmless and a lost push is not.
    fn all() -> [Push; 1] {
        [Push::Desk { tab: None }]
    }
}

/// Capacity of the daemon-wide [`Push`] bus.
pub(crate) const PUSH_CAP: usize = 32;

/// Put a push on the bus. A send with no open `/ws` is `Err`, and a push no
/// tab hears needs no message: a tab that opens reads every fact anyway.
pub(crate) fn push(pushes: &broadcast::Sender<Push>, p: Push) {
    let _ = pushes.send(p);
}

async fn send_push(socket: &mut WebSocket, verb: &str, payload: serde_json::Value) -> bool {
    let frame = Frame::Command(Command {
        id: 0,
        verb: verb.to_string(),
        payload,
    });
    socket
        .send(Message::Binary(protocol::encode(&frame).into()))
        .await
        .is_ok()
}

/// Build the presence heartbeat for the loaded identity and the daemon's
/// current uptime. `None` identity → a heartbeat with no name/avatar (the
/// daemon is alive but un-baptized).
pub(crate) fn build_presence(identity: Option<&identity::Identity>, uptime: Duration) -> Frame {
    Frame::Presence(Presence {
        name: identity.map(|i| i.name.clone()),
        avatar: identity.map(|i| i.avatar.clone()),
        uptime_secs: uptime.as_secs(),
    })
}

/// Push a presence heartbeat to a connected client every 2s until it hangs up
/// or the daemon shuts down. The send loop MUST exit on every teardown path —
/// a `None`/`Close`/error from the client (the `recv` arm) OR a daemon shutdown
/// (the `shutdown` arm) — and drop the socket, so no task keeps sending after a
/// disconnect and a held-open connection cannot stall graceful shutdown.
///
/// Between heartbeats it relays `sessions.dirty` from `sessions` and every
/// [`Push`] from `pushes`. A receiver that lagged sends every verb once; a
/// closed one turns its arm off, so the loop never spins on it.
pub(crate) async fn ws_presence_loop(
    mut socket: WebSocket,
    identity: Option<identity::Identity>,
    start: Instant,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    mut sessions: broadcast::Receiver<()>,
    mut pushes: broadcast::Receiver<Push>,
) {
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    let mut sessions_open = true;
    let mut pushes_open = true;
    loop {
        tokio::select! {
            changed = sessions.recv(), if sessions_open => match changed {
                Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    if !send_push(&mut socket, "sessions.dirty", serde_json::json!({})).await {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => sessions_open = false,
            },
            pushed = pushes.recv(), if pushes_open => match pushed {
                Ok(p) => {
                    if !send_push(&mut socket, p.verb(), p.payload()).await {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let mut sent = send_push(&mut socket, "sessions.dirty", serde_json::json!({})).await;
                    for p in Push::all() {
                        sent = sent && send_push(&mut socket, p.verb(), p.payload()).await;
                    }
                    if !sent {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => pushes_open = false,
            },
            // Ok = the daemon signalled shutdown; Err = the sender was dropped
            // (its runtime is going away). Either way, stop serving this socket.
            _ = shutdown.changed() => break,
            _ = tick.tick() => {
                let frame = build_presence(identity.as_ref(), start.elapsed());
                if socket
                    .send(Message::Binary(protocol::encode(&frame).into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            incoming = socket.recv() => {
                // None (stream closed), a Close frame, or a recv error all end
                // the loop; the socket drops when this task returns.
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}
