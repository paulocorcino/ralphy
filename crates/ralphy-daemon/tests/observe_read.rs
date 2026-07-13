//! The Observe read path answers on the requesting `Command` id and NEVER
//! spawns (issue #194; ADR-0036 §2/§4/§5): a `tree.list` returns the confined,
//! gitignore-filtered directory listing and a `file.read` of a binary file is
//! refused with a reason — both on the SAME `id`, with ZERO `status:"spawned"`
//! frames (no per-read `ralphy` process). Mirrors `tests/command_refusal.rs`.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

/// Bind a daemon over a temp repo seeded with `visible.txt`, `node_modules/junk`
/// and a binary `bin.dat`; return the `ws://…/ws/command` URL and the repo slug.
async fn serve_repo() -> (String, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("visible.txt"), b"hello").unwrap();
    std::fs::create_dir(dir.path().join("node_modules")).unwrap();
    std::fs::write(dir.path().join("node_modules/junk"), b"x").unwrap();
    std::fs::write(dir.path().join("bin.dat"), [0x00, 0x01, 0x02]).unwrap();

    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/observe";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    // Leak the tempdir so the registered repo outlives this fn (the daemon reads
    // it on every command); the OS reclaims it when the test process exits.
    std::mem::forget(dir);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthPolicy::Localhost,
    );
    // Leak the shutdown sender so the channel stays open for the server's lifetime.
    std::mem::forget(_tx);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        format!("ws://127.0.0.1:{port}/ws/command"),
        slug.to_string(),
    )
}

/// Send one `Command` and collect every reply frame on `id` until the socket
/// closes, returning `(replies, spawned_count)`.
async fn round_trip(
    url: &str,
    id: u64,
    verb: &str,
    payload: serde_json::Value,
) -> (Vec<serde_json::Value>, usize) {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connecting to /ws/command");
    ws.send(Message::Binary(protocol::encode(&Frame::Command(
        Command {
            id,
            verb: verb.to_string(),
            payload,
        },
    ))))
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        let mut replies = Vec::new();
        let mut spawned = 0usize;
        while let Some(msg) = ws.next().await {
            let bytes = match msg {
                Ok(Message::Binary(b)) => b,
                Ok(Message::Close(_)) | Err(_) => break,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.id != id {
                    continue;
                }
                if cmd.payload.get("status").and_then(|s| s.as_str()) == Some("spawned") {
                    spawned += 1;
                }
                replies.push(cmd.payload);
            }
        }
        (replies, spawned)
    })
    .await
    .expect("the reply must arrive and the socket close within 10s")
}

#[tokio::test]
async fn tree_list_answers_on_id_without_spawn() {
    let (url, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        1,
        "tree.list",
        serde_json::json!({ "repo": slug, "path": "" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "an Observe read must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok");
    let names: Vec<&str> = reply["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"visible.txt"), "names={names:?}");
    assert!(
        !names.contains(&"node_modules"),
        "noise filtered: {names:?}"
    );
}

#[tokio::test]
async fn file_read_refuses_binary() {
    let (url, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        2,
        "file.read",
        serde_json::json!({ "repo": slug, "path": "bin.dat" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "a refused read must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "error");
    let reason = reply["reason"].as_str().expect("a reason string");
    assert!(reason.contains("binary"), "reason={reason:?}");
}

#[tokio::test]
async fn file_read_masks_traversal_as_not_found() {
    // A `..` traversal over the wire must return a plain "not found", never
    // leaking whether the out-of-root target exists (ADR-0036 §5).
    let (url, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        3,
        "file.read",
        serde_json::json!({ "repo": slug, "path": "../secret" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "a refused read must never spawn");
    let reason = replies[0]["reason"].as_str().expect("a reason string");
    assert_eq!(replies[0]["status"], "error");
    assert!(reason.contains("not found"), "reason={reason:?}");
}
