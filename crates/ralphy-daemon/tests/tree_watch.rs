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
        ralphy_daemon::StorePaths::default(),
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

/// Send a `watch`/`unwatch` for `(repo, path)` under `checkout` (ADR-0063 §2).
async fn send_verb_checkout(ws: &mut Ws, verb: &str, repo: &str, path: &str, checkout: &str) {
    let frame = Frame::Command(Command {
        id: 0,
        verb: verb.to_string(),
        payload: serde_json::json!({ "repo": repo, "path": path, "checkout": checkout }),
    });
    ws.send(Message::Binary(protocol::encode(&frame)))
        .await
        .unwrap();
}

/// Wait up to 10s for a `tree.dirty` frame and return its `(repo, path,
/// checkout)` payload — `checkout` is `None` when the frame carries no key.
async fn recv_dirty(ws: &mut Ws) -> Option<(String, String, Option<String>)> {
    recv_dirty_within(ws, Duration::from_secs(10)).await
}

async fn recv_dirty_within(
    ws: &mut Ws,
    window: Duration,
) -> Option<(String, String, Option<String>)> {
    tokio::time::timeout(window, async {
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
                    let checkout = cmd.payload["checkout"].as_str().map(String::from);
                    return Some((repo, path, checkout));
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
        Some((slug.clone(), String::new(), None)),
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
        Some((slug.clone(), String::new(), None)),
        "the surviving client still receives nudges after the first disconnects"
    );
}

/// A `watch` under a `checkout` watches the WORKTREE's dir (prefixed under the
/// same root) and pushes the operator's rel back with the checkout name — so a
/// file written inside `.ralphy/worktrees/wt-a/` nudges a root-level watch of
/// `wt-a`, which a plain root watch never sees (non-recursive).
#[tokio::test]
async fn dirty_nudge_reaches_a_checkout_watcher() {
    let (url, slug, root) = serve_repo().await;
    let wt = root.join(".ralphy/worktrees/wt-a");
    std::fs::create_dir_all(&wt).unwrap();
    let gitdir = root
        .join(".git/worktrees/wt-a")
        .to_string_lossy()
        .replace('\\', "/");
    std::fs::write(wt.join(".git"), format!("gitdir: {gitdir}\n")).unwrap();

    let (mut ws, _resp) = connect_async(&url).await.expect("connect /ws/tree");
    send_verb_checkout(&mut ws, "watch", &slug, "", "wt-a").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    std::fs::write(wt.join("f.txt"), b"hello").unwrap();

    let got = recv_dirty(&mut ws).await;
    assert_eq!(
        got,
        Some((slug.clone(), String::new(), Some("wt-a".to_string()))),
        "a create inside the worktree nudges the checkout watcher with the operator's rel"
    );
}

/// `unwatch` under a `checkout` releases the alias with the dir: a later plain
/// watch of the same PREFIXED rel is pushed in its own form (the prefixed
/// path, no `checkout`), not in the retired alias's.
#[tokio::test]
async fn unwatch_with_checkout_retires_the_alias() {
    let (url, slug, root) = serve_repo().await;
    let wt = root.join(".ralphy/worktrees/wt-a");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(wt.join(".git"), "gitdir: /r/.git/worktrees/wt-a\n").unwrap();

    let (mut ws, _resp) = connect_async(&url).await.expect("connect /ws/tree");
    send_verb_checkout(&mut ws, "watch", &slug, "", "wt-a").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    send_verb_checkout(&mut ws, "unwatch", &slug, "", "wt-a").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    send_verb(&mut ws, "watch", &slug, ".ralphy/worktrees/wt-a").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    std::fs::write(wt.join("g.txt"), b"x").unwrap();
    let got = recv_dirty(&mut ws).await;
    assert_eq!(
        got,
        Some((slug.clone(), ".ralphy/worktrees/wt-a".to_string(), None)),
        "the retired alias must not relabel a plain watch of the same dir"
    );
}

/// A present-but-malformed `checkout` (`""`, a number) drops the frame — the
/// socket never silently holds a PRIMARY watch the client believes is the
/// worktree's. Same door as `checkout::from_payload`.
#[tokio::test]
async fn malformed_checkout_on_watch_holds_nothing() {
    let (url, slug, root) = serve_repo().await;
    let (mut ws, _resp) = connect_async(&url).await.expect("connect /ws/tree");
    for checkout in [serde_json::json!(""), serde_json::json!(5)] {
        let frame = Frame::Command(Command {
            id: 0,
            verb: "watch".to_string(),
            payload: serde_json::json!({ "repo": slug, "path": "", "checkout": checkout }),
        });
        ws.send(Message::Binary(protocol::encode(&frame)))
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    std::fs::write(root.join("h.txt"), b"x").unwrap();
    let got = recv_dirty_within(&mut ws, Duration::from_secs(3)).await;
    assert_eq!(got, None, "no watch was held for a malformed checkout");

    // POSITIVE CONTROL on the same socket: a well-formed plain watch still works.
    send_verb(&mut ws, "watch", &slug, "").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    std::fs::write(root.join("i.txt"), b"x").unwrap();
    assert_eq!(
        recv_dirty(&mut ws).await,
        Some((slug.clone(), String::new(), None))
    );
}
