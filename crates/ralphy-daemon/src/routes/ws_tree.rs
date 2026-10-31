//! `/ws/tree`: the live file-tree socket, its peer poller and the run-exit
//! nudge relay (#196, #310).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::routing::get;
use axum::Router;
use futures_util::StreamExt;

use super::send_command_on;
use super::RouterShared;
use crate::mux::{ChannelSender, Mode, Writer};
use crate::watch;

mod conn;
mod frame;
mod peer_poll;
pub(crate) use conn::TreeConn;
pub(crate) use peer_poll::*;

/// What the daemon lends to one `/ws/tree` connection: the watchers, the stores
/// it reads and this daemon's identity.
pub(crate) struct TreeHost {
    pub(crate) watchers: Arc<watch::WatcherManager>,
    pub(crate) registry_path: PathBuf,
    pub(crate) peers_dir: PathBuf,
    pub(crate) daemon_id: Option<String>,
}

/// `GET /ws/tree`: the persistent live-tree subscription socket (#196, ADR-0036
/// §4). A client `Frame::Command{verb:"watch", payload:{repo,path}}` starts
/// watching that repo dir (subscribing to the repo's nudge broadcast on the first
/// watch); `verb:"unwatch"` releases it. A settled change on a watched dir is
/// pushed back as `Frame::Command{verb:"tree.dirty", payload:{repo,path}}`, and
/// the browser re-reads that subtree via the Observe `tree.list` path.
///
/// The socket carries a SECOND subscription kind (#300, ADR-0047 §9): `runs.watch`
/// / `runs.unwatch` hold [`watch::RUNSTATE_REL`], the repo's run-snapshot dir, and
/// a change there pushes `runs.dirty {repo}` — the browser re-reads `runs.list`.
/// Both kinds live in the SAME `watched` list, so the teardown below releases them
/// identically.
///
/// A THIRD push kind (#310, ADR-0036 amendment) is NOT watcher-fed: `changes.dirty
/// {repo}` relays the daemon-wide run-exit broadcast, so it has no subscription
/// verb, no entry in `watched`, and nothing to release — the browser filters it by
/// the repo it has open.
///
/// A FOURTH kind, `head.watch { repo, checkout? }`, holds the checkout's gitdir
/// and its `logs/` (`checkout::head_rels`); the watcher lets only their `HEAD`
/// child through, and a move pushes `head.dirty { repo, checkout? }`. Local
/// repos only. It has no unwatch verb: the browser holds it on the tree's own
/// socket, which it closes when the checkout or the project changes.
///
/// TEARDOWN INVARIANT: on EVERY exit path — daemon shutdown OR client close/error
/// — the connection releases EVERY dir it watched (tracked in `watched`) so the
/// last release tears the repo watcher down, and aborts its forwarder tasks. A
/// leaked watch would keep an OS watcher (and its debouncer thread) alive forever.
///
/// The optional `checkout` argument (ADR-0063 §2) is applied LEXICALLY — the shape
/// gate only, no pointer-file read: a watch has no reply frame to say `unknown
/// checkout` in, and the `tree.list` of the same level that precedes every
/// watch does. The watched rel is PREFIXED under the same root (so the manager,
/// `watched`, the refcount and the teardown see only prefixed rels, unchanged),
/// and a per-connection alias map turns the nudge back into the operator's
/// coordinates: `tree.dirty { repo, path: <operator rel>, checkout: <name> }`.
pub(crate) async fn tree_ws(
    socket: WebSocket,
    host: TreeHost,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    run_exits: tokio::sync::broadcast::Sender<String>,
) {
    let (nudge_tx, mut nudge_rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    let (sink, mut incoming_frames) = socket.split();
    // Channel 0 is the socket itself: its frames leave whole and bare.
    let writer = Writer::start(sink, Mode::Plain);
    let out = writer.channel(0);
    let mut conn = TreeConn::new(host, nudge_tx, out.clone());
    // The run-completion nudge bus (#310): daemon-wide, held by NO watch, so it
    // needs no subscription verb and adds nothing to the teardown. Every
    // connection relays every nudge; the browser filters by its open repo.
    let mut run_exits_rx = run_exits.subscribe();
    let mut exits_open = true;

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            incoming = incoming_frames.next() => match incoming {
                Some(Ok(Message::Binary(bytes))) => conn.on_frame(&bytes).await,
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            nudge = nudge_rx.recv() => conn.push_nudge(nudge).await,
            exited = run_exits_rx.recv(), if exits_open => {
                exits_open = relay_run_exit(&out, exited).await;
            }
        }
    }

    conn.teardown();
    // A client that reads nothing keeps the writer blocked in the socket
    // send, so the writer is stopped here, not left to the dropped handle.
    writer.abort();
}

/// Queue one push on the connection's channel. A closed writer means the
/// socket is gone, and the loop's read side ends the connection.
async fn push_command(out: &ChannelSender, verb: &str, payload: serde_json::Value) {
    if send_command_on(out, 0, verb, payload).await.is_err() {
        tracing::debug!(verb, "tree push dropped: the socket writer is closed");
    }
}

/// Relay one run-exit nudge as `changes.dirty`. Returns whether the broadcast is
/// still open.
pub(crate) async fn relay_run_exit(
    out: &ChannelSender,
    exited: Result<String, tokio::sync::broadcast::error::RecvError>,
) -> bool {
    match exited {
        Ok(repo) => {
            push_command(out, "changes.dirty", serde_json::json!({ "repo": repo })).await;
            true
        }
        // Skipped nudges are free: the browser's re-read is idempotent.
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
        // Load-bearing: a closed broadcast makes `recv()` return
        // immediately forever, which would spin the loop.
        Err(tokio::sync::broadcast::error::RecvError::Closed) => false,
    }
}

/// Pipe one repo's `tree.dirty` broadcast into the connection's fan-in channel.
/// A lag just skips ahead (the browser re-reads idempotently); a closed broadcast
/// (the repo watcher torn down) or a dropped fan-in ends the task.
pub(crate) fn spawn_nudge_forwarder(
    mut rx: watch::DirtyRx,
    tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(item) => {
                    if tx.send(item).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// `/ws/tree`: the file-tree socket.
pub(crate) fn tree_routes(s: &RouterShared) -> Router {
    let watchers = s.watchers.clone();
    let registry = s.registry_path.clone();
    let peers = s.peers_dir.clone();
    let daemon_id = s.daemon_id.clone();
    let shutdown = s.shutdown.clone();
    let run_exits = s.run_exits.clone();
    Router::new().route(
        "/ws/tree",
        get(move |ws: WebSocketUpgrade| {
            let host = TreeHost {
                watchers: watchers.clone(),
                registry_path: registry.clone(),
                peers_dir: peers.clone(),
                daemon_id: daemon_id.clone(),
            };
            let shutdown = shutdown.clone();
            let run_exits = run_exits.clone();
            async move { ws.on_upgrade(move |socket| tree_ws(socket, host, shutdown, run_exits)) }
        }),
    )
}
