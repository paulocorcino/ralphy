//! The Write byte-op path answers on the requesting `Command` id and NEVER spawns
//! (issue #197; ADR-0036 Write amendment): `file.write`/`create`/`rename`/`delete`
//! perform a confined byte-op in-daemon and reply once on the SAME `id`, with ZERO
//! `status:"spawned"` frames. A write-escape (traversal or symlink) is refused
//! verbatim as `reason:"refused"` and touches NOTHING outside the root. Mirrors
//! `tests/observe_read.rs`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

/// Bind a daemon over a temp repo seeded with `a.txt`; return the
/// `ws://…/ws/command` URL, the repo slug, AND the leaked repo root so a test can
/// assert on-disk state after a Write.
async fn serve_repo() -> (String, String, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
    let root = dir.path().to_path_buf();

    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/workspace";
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
    std::mem::forget(_tx);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        format!("ws://127.0.0.1:{port}/ws/command"),
        slug.to_string(),
        root,
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
async fn write_persists_bytes() {
    let (url, slug, root) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        1,
        "file.write",
        serde_json::json!({ "repo": slug, "path": "note.txt", "content": "hi" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "a Write must never spawn");
    assert_eq!(replies[0]["status"], "ok");
    assert_eq!(
        std::fs::read_to_string(root.join("note.txt")).unwrap(),
        "hi",
        "the bytes hit disk"
    );
}

#[tokio::test]
async fn create_folder() {
    let (url, slug, root) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        2,
        "file.create",
        serde_json::json!({ "repo": slug, "path": "newdir", "dir": true }),
    )
    .await;

    assert_eq!(replies.len(), 1);
    assert_eq!(spawned, 0);
    assert_eq!(replies[0]["status"], "ok");
    assert!(root.join("newdir").is_dir(), "the dir was created");
}

#[tokio::test]
async fn rename_moves() {
    let (url, slug, root) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        3,
        "file.rename",
        serde_json::json!({ "repo": slug, "path": "a.txt", "to": "b.txt" }),
    )
    .await;

    assert_eq!(replies.len(), 1);
    assert_eq!(spawned, 0);
    assert_eq!(replies[0]["status"], "ok");
    assert!(!root.join("a.txt").exists(), "the source is gone");
    assert!(root.join("b.txt").exists(), "the dest is present");
}

#[tokio::test]
async fn delete_removes() {
    let (url, slug, root) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        4,
        "file.delete",
        serde_json::json!({ "repo": slug, "path": "a.txt" }),
    )
    .await;

    assert_eq!(replies.len(), 1);
    assert_eq!(spawned, 0);
    assert_eq!(replies[0]["status"], "ok");
    assert!(!root.join("a.txt").exists(), "the file is removed");
}

#[tokio::test]
async fn write_escape_refused() {
    let (url, slug, root) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        5,
        "file.write",
        serde_json::json!({ "repo": slug, "path": "../evil", "content": "boom" }),
    )
    .await;

    assert_eq!(replies.len(), 1);
    assert_eq!(spawned, 0, "a refused write must never spawn");
    assert_eq!(replies[0]["status"], "error");
    assert_eq!(replies[0]["reason"], "refused", "surfaced verbatim, not masked");
    assert!(
        !root.parent().unwrap().join("evil").exists(),
        "nothing written outside the root"
    );
}

#[tokio::test]
async fn create_conflict() {
    let (url, slug, _root) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        6,
        "file.create",
        serde_json::json!({ "repo": slug, "path": "a.txt", "dir": false }),
    )
    .await;

    assert_eq!(replies.len(), 1);
    assert_eq!(spawned, 0);
    assert_eq!(replies[0]["status"], "error");
    assert_eq!(replies[0]["reason"], "exists");
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_write_escape_refused() {
    use std::os::unix::fs::symlink;
    let (url, slug, root) = serve_repo().await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("target.txt"), b"secret").unwrap();
    // An in-root symlink pointing at an outside file: a write THROUGH it must be
    // refused and the outside target left untouched.
    symlink(outside.path().join("target.txt"), root.join("link.txt")).unwrap();

    let (replies, spawned) = round_trip(
        &url,
        7,
        "file.write",
        serde_json::json!({ "repo": slug, "path": "link.txt", "content": "boom" }),
    )
    .await;

    assert_eq!(replies.len(), 1);
    assert_eq!(spawned, 0);
    assert_eq!(replies[0]["status"], "error");
    assert_eq!(replies[0]["reason"], "refused");
    assert_eq!(
        std::fs::read_to_string(outside.path().join("target.txt")).unwrap(),
        "secret",
        "the outside target's bytes are unchanged"
    );
}
