//! The `tree` and `runs` kinds: each channel is one `/ws/tree` connection.
//!
//! The page sends the verbs of `/ws/tree` on the channel (`watch`,
//! `runs.watch`, …) and the pushes come back on it. The two kinds are
//! the same connection under two names, so a page keeps its file-tree
//! watches and its run watches on separate channels.

use tokio::sync::{broadcast, mpsc};

use super::{Channel, CloseReason, Opening};
use crate::mux::ChannelSender;
use crate::routes::{TreeConn, TreeHost};

/// Serve a `tree` channel.
pub(super) fn open(
    opening: Opening,
    host: TreeHost,
    _run_exits: broadcast::Sender<String>,
) -> Result<Channel, CloseReason> {
    Ok(serve_channel(opening, host))
}

/// Serve a `runs` channel.
pub(super) fn open_runs(
    opening: Opening,
    host: TreeHost,
    _run_exits: broadcast::Sender<String>,
) -> Result<Channel, CloseReason> {
    Ok(serve_channel(opening, host))
}

fn serve_channel(opening: Opening, host: TreeHost) -> Channel {
    let out = opening.out;
    Channel::spawn(move |frames| serve(frames, host, out))
}

/// Run one connection until the page closes the channel. The connection
/// releases its watches when it is dropped, so a task that is stopped from
/// outside, as closing the channel does, releases them too.
async fn serve(mut frames: mpsc::Receiver<Vec<u8>>, host: TreeHost, out: ChannelSender) {
    let (nudge_tx, mut nudge_rx) = mpsc::unbounded_channel::<(String, String)>();
    let mut conn = TreeConn::new(host, nudge_tx, out);
    loop {
        tokio::select! {
            frame = frames.recv() => match frame {
                Some(bytes) => conn.on_frame(&bytes).await,
                None => break,
            },
            nudge = nudge_rx.recv() => conn.push_nudge(nudge).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::extract::ws::WebSocketUpgrade;
    use futures_util::StreamExt;

    use super::*;
    use crate::mux::{Mode, Writer};
    use crate::protocol::{self, Command, Frame};
    use crate::{registry, watch};

    const SLUG: &str = "owner/tree";

    /// Serve one page socket whose only channel is a `tree` channel, and
    /// answer the channel with the writer that keeps its socket open.
    async fn serve_channel_over_a_socket(host: TreeHost) -> (Channel, Writer, impl std::any::Any) {
        let (give, take) = tokio::sync::oneshot::channel::<(Channel, Writer)>();
        let held = Arc::new(Mutex::new(Some((host, give))));
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(move |ws: WebSocketUpgrade| {
                let (host, give) = held
                    .lock()
                    .expect("held mutex")
                    .take()
                    .expect("one client connects");
                async move {
                    ws.on_upgrade(move |socket| async move {
                        let (sink, _stream) = socket.split();
                        let writer = Writer::start(sink, Mode::Channels);
                        let out = writer.channel(1);
                        let channel = Channel::spawn(move |frames| serve(frames, host, out));
                        give.send((channel, writer))
                            .unwrap_or_else(|_| panic!("the test waits for the channel"));
                    })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let port = listener.local_addr().expect("bound address").port();
        tokio::spawn(async move { axum::serve(listener, app).await });
        let (client, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/"))
            .await
            .expect("connect to the test socket");
        let (channel, writer) = take.await.expect("the server starts the channel");
        (channel, writer, client)
    }

    async fn refcount_reaches(watchers: &watch::WatcherManager, want: u32) -> bool {
        for _ in 0..50 {
            if watchers.watch_refcount(SLUG, "") == want {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// A channel that is stopped from outside, as closing it does, releases
    /// the watches its connection holds.
    #[tokio::test]
    async fn stopping_a_tree_channel_releases_its_watches() {
        let dir = tempfile::tempdir().expect("create the temp dir");
        let registry_path = dir.path().join("repos.toml");
        let mut store = registry::RegistryStore::default();
        store.upsert(SLUG, &dir.path().to_string_lossy());
        registry::save_to(&store, &registry_path).expect("save the registry");
        let watchers = Arc::new(watch::WatcherManager::new(watch::MAX_WATCHES));
        let host = TreeHost {
            watchers: watchers.clone(),
            registry_path,
            peers_dir: dir.path().join("peers"),
            daemon_id: None,
        };
        let (channel, _writer, _client) = serve_channel_over_a_socket(host).await;

        let watch_frame = protocol::encode(&Frame::Command(Command {
            id: 0,
            verb: "watch".to_string(),
            payload: serde_json::json!({ "repo": SLUG, "path": "" }),
        }));
        assert!(channel.take(watch_frame).await, "the channel takes frames");
        assert!(refcount_reaches(&watchers, 1).await, "the watch is held");

        drop(channel);

        assert!(
            refcount_reaches(&watchers, 0).await,
            "stopping the channel releases its watch"
        );
    }
}
