//! The HEAD watch over the wire: a `/ws/tree` client that sends `head.watch`
//! receives `head.dirty` when real git moves the checkout's HEAD, and nothing
//! for a `git status` — whose `index` rewrite, answered by another
//! `git status`, would otherwise loop. Mirrors `tests/runs_watch.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git (CI and the build machine have git)");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Bind a daemon over a temp repo with one commit; return the `/ws/tree` URL,
/// the slug and the root.
async fn serve_repo() -> (String, String, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    git(&root, &["add", "a.txt"]);
    git(&root, &["commit", "-q", "-m", "first"]);

    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/head";
    store.upsert(slug, &root.to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    // Leak the tempdir so the registered repo outlives this fn.
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

async fn send_head_watch(ws: &mut Ws, repo: &str, checkout: Option<&str>) {
    let mut payload = serde_json::json!({ "repo": repo, "path": "" });
    if let Some(name) = checkout {
        payload["checkout"] = serde_json::json!(name);
    }
    let frame = Frame::Command(Command {
        id: 0,
        verb: "head.watch".to_string(),
        payload,
    });
    ws.send(Message::Binary(protocol::encode(&frame).into()))
        .await
        .unwrap();
}

/// The payload of the first `head.dirty` within `dur`, if any.
async fn recv_head(ws: &mut Ws, dur: Duration) -> Option<serde_json::Value> {
    tokio::time::timeout(dur, async {
        while let Some(msg) = ws.next().await {
            let bytes = match msg {
                Ok(Message::Binary(b)) => b,
                Ok(Message::Close(_)) | Err(_) => return None,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.verb == "head.dirty" {
                    return Some(cmd.payload);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// Read and drop every `head.dirty` until a quiet gap.
async fn drain(ws: &mut Ws) {
    while recv_head(ws, Duration::from_secs(2)).await.is_some() {}
}

#[tokio::test]
async fn a_branch_switch_pushes_head_dirty_and_git_status_does_not() {
    let (url, slug, root) = serve_repo().await;
    let (mut ws, _resp) = connect_async(&url).await.expect("connect /ws/tree");
    send_head_watch(&mut ws, &slug, None).await;
    // Give the OS watch a beat to attach, then settle anything it replayed.
    tokio::time::sleep(Duration::from_millis(500)).await;
    drain(&mut ws).await;

    git(&root, &["status", "--porcelain"]);
    std::fs::write(root.join("a.txt"), b"changed").unwrap();
    git(&root, &["status", "--porcelain"]);
    assert_eq!(
        recv_head(&mut ws, Duration::from_secs(3)).await,
        None,
        "git status rewrites the index, which is not a HEAD move"
    );

    git(&root, &["switch", "-q", "-c", "feature"]);
    assert_eq!(
        recv_head(&mut ws, Duration::from_secs(10)).await,
        Some(serde_json::json!({ "repo": slug })),
        "a branch switch pushes head.dirty for the primary"
    );
}

#[tokio::test]
async fn a_commit_pushes_head_dirty() {
    let (url, slug, root) = serve_repo().await;
    let (mut ws, _resp) = connect_async(&url).await.expect("connect /ws/tree");
    send_head_watch(&mut ws, &slug, None).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    drain(&mut ws).await;

    // A commit on the same branch leaves `HEAD` alone and appends to `logs/HEAD`.
    git(&root, &["commit", "-q", "--allow-empty", "-m", "second"]);
    assert_eq!(
        recv_head(&mut ws, Duration::from_secs(10)).await,
        Some(serde_json::json!({ "repo": slug })),
    );
}

#[tokio::test]
async fn a_worktree_switch_pushes_head_dirty_with_its_name() {
    let (url, slug, root) = serve_repo().await;
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt-a",
            ".ralphy/worktrees/wt-a",
        ],
    );
    let wt = root.join(".ralphy/worktrees/wt-a");

    let (mut ws, _resp) = connect_async(&url).await.expect("connect /ws/tree");
    send_head_watch(&mut ws, &slug, Some("wt-a")).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    drain(&mut ws).await;

    git(&wt, &["switch", "-q", "-c", "wt-b"]);
    assert_eq!(
        recv_head(&mut ws, Duration::from_secs(10)).await,
        Some(serde_json::json!({ "repo": slug, "checkout": "wt-a" })),
        "the worktree's own HEAD is watched, and the push names it"
    );
}
