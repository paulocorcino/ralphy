//! `/ws`: the page socket, with the presence heartbeat and the pushes on
//! channel 0. The socket itself is served in `mux`.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::WebSocketUpgrade;
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Extension, Router};
use tokio::sync::broadcast;

use super::{request_may_carry_a_secret, Caller, RouterShared, SessionHost, SocketAudit};
use crate::protocol::{Frame, Presence};
use crate::{identity, mux, presence_board};

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
    /// The attention list changed (`attention::tick`).
    Attention,
}

impl Push {
    pub(crate) fn verb(&self) -> &'static str {
        match self {
            Push::Desk { .. } => "desk.dirty",
            Push::Repos => "repos.dirty",
            Push::Peers => "peers.dirty",
            Push::Attention => "attention.dirty",
        }
    }

    pub(crate) fn payload(&self) -> serde_json::Value {
        match self {
            Push::Desk { tab } => serde_json::json!({ "tab": tab }),
            Push::Repos | Push::Peers | Push::Attention => serde_json::json!({}),
        }
    }

    /// Every push once: what a receiver that lagged sends, because a read
    /// again is harmless and a lost push is not.
    pub(crate) fn all() -> [Push; 4] {
        [
            Push::Desk { tab: None },
            Push::Repos,
            Push::Peers,
            Push::Attention,
        ]
    }
}

/// The push every page socket sends when the console list changes. The
/// session manager's own bus carries it, not the [`Push`] bus.
pub(crate) const SESSIONS_DIRTY: &str = "sessions.dirty";

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
    let Some(mut last) = stamp_stores(&registry_path, &peers_dir).await else {
        return;
    };
    let mut tick = tokio::time::interval(STORE_STAMP_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await;
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = tick.tick() => {
                let Some(now) = stamp_stores(&registry_path, &peers_dir).await else {
                    break;
                };
                push_changed(&pushes, &last, &now);
                last = now;
            }
        }
    }
}

/// Push `repos.dirty`, then `peers.dirty`, for each stamp that changed.
fn push_changed(pushes: &broadcast::Sender<Push>, last: &(String, String), now: &(String, String)) {
    if now.0 != last.0 {
        push(pushes, Push::Repos);
    }
    if now.1 != last.1 {
        push(pushes, Push::Peers);
    }
}

/// The stamps of `repos.toml` and of the peer store, or `None` when the
/// blocking task that reads them failed, which ends the watch.
async fn stamp_stores(registry_path: &Path, peers_dir: &Path) -> Option<(String, String)> {
    let (r, p) = (registry_path.to_path_buf(), peers_dir.to_path_buf());
    match tokio::task::spawn_blocking(move || (store_stamp(&r), peers_stamp(&p))).await {
        Ok(stamps) => Some(stamps),
        Err(e) => {
            tracing::warn!(error = %e, "the store watch stopped; repos and peers are no longer pushed");
            None
        }
    }
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

/// `/ws`: the page socket. Channel 0 is the presence heartbeat and the
/// pushes; the page opens its other channels on it.
pub(crate) fn presence_routes(s: &RouterShared) -> Router {
    let page = PageShared {
        start: s.start,
        pushes: s.pushes.clone(),
        board: s.presence.clone(),
        audit: s.audit.clone(),
        host: SessionHost {
            peers_dir: s.peers_dir.clone(),
            identity: s.identity.clone(),
            environment: s.environment.clone(),
            bound_port: s.bound_port,
            sessions: s.sessions.clone(),
            registry_path: s.registry_path.clone(),
            shutdown: s.shutdown.clone(),
        },
        daemon_id: s.daemon_id.clone(),
        watchers: s.watchers.clone(),
        run_exits: s.run_exits.clone(),
        auth: s.auth.clone(),
    };
    Router::new().route(
        "/ws",
        get(
            move |ws: WebSocketUpgrade, caller: Option<Extension<Caller>>, headers: HeaderMap| {
                let page = page.page(caller, &headers);
                // A bare frame keeps its own smaller cap in the reader.
                async move {
                    ws.max_message_size(mux::MAX_MESSAGE)
                        .max_frame_size(mux::MAX_MESSAGE)
                        .on_upgrade(move |socket| mux::serve(socket, page))
                }
            },
        ),
    )
}

/// What every page socket of the router serves from, cloned per socket.
#[derive(Clone)]
struct PageShared {
    start: Instant,
    pushes: broadcast::Sender<Push>,
    board: presence_board::Shared,
    audit: Arc<crate::audit::Audit>,
    host: SessionHost,
    daemon_id: Option<String>,
    watchers: Arc<crate::watch::WatcherManager>,
    run_exits: broadcast::Sender<String>,
    auth: Arc<crate::auth::AuthState>,
}

impl PageShared {
    /// The page socket of one request.
    fn page(&self, caller: Option<Extension<Caller>>, headers: &HeaderMap) -> mux::Page {
        mux::Page {
            identity: self.host.identity.clone(),
            start: self.start,
            shutdown: self.host.shutdown.clone(),
            sessions: self.host.sessions.subscribe_changes(),
            pushes: self.pushes.subscribe(),
            board: self.board.clone(),
            ctx: mux::Ctx {
                who: SocketAudit::of(self.audit.clone(), caller),
                secret_ok: request_may_carry_a_secret(headers),
                host: self.host.clone(),
                daemon_id: self.daemon_id.clone(),
                watchers: self.watchers.clone(),
                run_exits: self.run_exits.clone(),
            },
            login: mux::Login::now(self.auth.clone()),
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
