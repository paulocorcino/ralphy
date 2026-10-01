//! `/ws`: the presence heartbeat.

use std::path::Path;
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
    /// `repos.toml` changed on disk (`ralphy daemon add` writes it from
    /// another process).
    Repos,
    /// The peer store changed on disk (`ralphy host …` writes it from
    /// another process).
    Peers,
}

impl Push {
    fn verb(&self) -> &'static str {
        match self {
            Push::Desk { .. } => "desk.dirty",
            Push::Repos => "repos.dirty",
            Push::Peers => "peers.dirty",
        }
    }

    fn payload(&self) -> serde_json::Value {
        match self {
            Push::Desk { tab } => serde_json::json!({ "tab": tab }),
            Push::Repos | Push::Peers => serde_json::json!({}),
        }
    }

    /// Every push once: what a receiver that lagged sends, because a read
    /// again is harmless and a lost push is not.
    fn all() -> [Push; 3] {
        [Push::Desk { tab: None }, Push::Repos, Push::Peers]
    }
}

/// Capacity of the daemon-wide [`Push`] bus.
pub(crate) const PUSH_CAP: usize = 32;

/// Put a push on the bus. A send with no open `/ws` is `Err`, and a push no
/// tab hears needs no message: a tab that opens reads every fact anyway.
pub(crate) fn push(pushes: &broadcast::Sender<Push>, p: Push) {
    if pushes.send(p).is_err() {
        // No open `/ws`: nothing to tell.
    }
}

/// How often [`watch_stores`] looks at the files other processes write.
pub(crate) const STORE_STAMP_EVERY: Duration = Duration::from_secs(2);

/// A stamp of one file: its change time and length, or why it cannot be
/// read. Two equal stamps mean "no change seen".
pub(crate) fn store_stamp(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(m) => {
            let modified = m
                .modified()
                .map(|t| format!("{t:?}"))
                .unwrap_or_else(|e| e.to_string());
            format!("{modified}/{}", m.len())
        }
        Err(e) => format!("error: {e}"),
    }
}

/// A stamp of a directory: the sorted name, change time and length of each
/// entry, or why it cannot be read.
pub(crate) fn peers_stamp(dir: &Path) -> String {
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            let mut rows: Vec<String> = entries
                .map(|entry| match entry {
                    Ok(entry) => format!(
                        "{}:{}",
                        entry.file_name().to_string_lossy(),
                        store_stamp(&entry.path())
                    ),
                    Err(e) => format!("error: {e}"),
                })
                .collect();
            rows.sort();
            rows.join("\n")
        }
        Err(e) => format!("error: {e}"),
    }
}

/// Push `repos.dirty` / `peers.dirty` when `repos.toml` or the peer store
/// changes on disk. Another process writes both files, so only the files show
/// the change; a stat every [`STORE_STAMP_EVERY`] works the same on every OS
/// and needs no watcher thread. Ends on daemon shutdown.
pub(crate) async fn watch_stores(
    registry_path: std::path::PathBuf,
    peers_dir: std::path::PathBuf,
    pushes: broadcast::Sender<Push>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let stamp = |r: std::path::PathBuf, p: std::path::PathBuf| {
        tokio::task::spawn_blocking(move || (store_stamp(&r), peers_stamp(&p)))
    };
    let mut last = match stamp(registry_path.clone(), peers_dir.clone()).await {
        Ok(stamps) => stamps,
        Err(e) => {
            tracing::warn!(error = %e, "the store watch stopped; repos and peers are no longer pushed");
            return;
        }
    };
    let mut tick = tokio::time::interval(STORE_STAMP_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await;
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = tick.tick() => {
                let now = match stamp(registry_path.clone(), peers_dir.clone()).await {
                    Ok(stamps) => stamps,
                    Err(e) => {
                        tracing::warn!(error = %e, "the store watch stopped; repos and peers are no longer pushed");
                        break;
                    }
                };
                if now.0 != last.0 {
                    push(&pushes, Push::Repos);
                }
                if now.1 != last.1 {
                    push(&pushes, Push::Peers);
                }
                last = now;
            }
        }
    }
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
        build: Some(crate::assets::build_id().to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_changes_when_the_store_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("repos.toml");
        std::fs::write(&file, "a = 1\n").unwrap();
        let first = store_stamp(&file);
        assert_eq!(
            store_stamp(&file),
            first,
            "an unchanged file keeps its stamp"
        );
        std::fs::write(&file, "a = 1\nb = 2\n").unwrap();
        assert_ne!(store_stamp(&file), first, "a rewrite changes the stamp");

        let peers = dir.path().join("peers");
        let empty = peers_stamp(&peers);
        std::fs::create_dir(&peers).unwrap();
        std::fs::write(peers.join("x.toml"), "x").unwrap();
        let one = peers_stamp(&peers);
        assert_ne!(one, empty, "a new descriptor changes the stamp");
        assert_eq!(peers_stamp(&peers), one);
    }
}
