//! The live file-tree watcher over the wire (issue #196; ADR-0036 §4): a
//! `/ws/tree` client that `watch`es a repo dir receives a `tree.dirty` nudge when
//! a file changes there, and the per-repo watcher is SHARED — a second client on
//! the same dir keeps receiving after the first disconnects. Mirrors
//! `tests/observe_read.rs`'s `serve_repo` harness, but the URL is `…/ws/tree` and
//! the socket stays OPEN (a subscription, not one answer-and-close).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Bind a daemon over an empty temp repo; return the `ws://…/ws/tree` URL, the
/// repo slug, and the repo root path (so the test can create files in it).
async fn serve_repo() -> (String, String, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/tree";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    // Leak the tempdir so the registered repo outlives this fn (the daemon reads
    // it on every command); the OS reclaims it when the test process exits.
    std::mem::forget(dir);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    // Leak the shutdown sender so the channel stays open for the server's lifetime.
    std::mem::forget(tx);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        format!("ws://127.0.0.1:{port}/ws/tree"),
        slug.to_string(),
        root,
    )
}

/// Send a `watch`/`unwatch` command for `(repo, path)` over the open socket.
async fn send_verb(ws: &mut Ws, verb: &str, repo: &str, path: &str) {
    let frame = Frame::Command(Command {
        id: 0,
        verb: verb.to_string(),
        payload: serde_json::json!({ "repo": repo, "path": path }),
    });
    ws.send(Message::Binary(protocol::encode(&frame)))
        .await
        .unwrap();
}

/// Wait up to 10s for a `tree.dirty` frame and return its `(repo, path)` payload.
async fn recv_dirty(ws: &mut Ws) -> Option<(String, String)> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(msg) = ws.next().await {
            let bytes = match msg {
                Ok(Message::Binary(b)) => b,
                Ok(Message::Close(_)) | Err(_) => return None,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.verb == "tree.dirty" {
                    let repo = cmd.payload["repo"].as_str().unwrap_or("").to_string();
                    let path = cmd.payload["path"].as_str().unwrap_or("").to_string();
                    return Some((repo, path));
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

#[tokio::test]
async fn dirty_nudge_reaches_a_watcher() {
    let (url, slug, root) = serve_repo().await;
    let (mut ws, _resp) = connect_async(&url).await.expect("connect /ws/tree");
    send_verb(&mut ws, "watch", &slug, "").await;
    // Let the server establish the OS watch before the change that must be caught.
    tokio::time::sleep(Duration::from_millis(500)).await;

    std::fs::write(root.join("f.txt"), b"hello").unwrap();

    let got = recv_dirty(&mut ws).await;
    assert_eq!(
        got,
        Some((slug.clone(), String::new())),
        "a watched-root create nudges"
    );
}

#[tokio::test]
async fn shared_across_clients_survives_one_disconnect() {
    let (url, slug, root) = serve_repo().await;
    let (mut ws1, _r1) = connect_async(&url).await.expect("connect client 1");
    let (mut ws2, _r2) = connect_async(&url).await.expect("connect client 2");
    send_verb(&mut ws1, "watch", &slug, "").await;
    send_verb(&mut ws2, "watch", &slug, "").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Client 1 leaves; the per-repo watcher is shared, so client 2's subscription
    // must survive (refcount 2 → 1, the repo watcher stays alive).
    drop(ws1);
    tokio::time::sleep(Duration::from_millis(300)).await;

    std::fs::write(root.join("after.txt"), b"x").unwrap();

    let got = recv_dirty(&mut ws2).await;
    assert_eq!(
        got,
        Some((slug.clone(), String::new())),
        "the surviving client still receives nudges after the first disconnects"
    );
}
