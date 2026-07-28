//! The run-snapshot subscription over the wire (issue #300; ADR-0047 §9): a
//! `/ws/tree` client that sends `runs.watch` receives a `runs.dirty` push when a
//! snapshot document under the repo's `.ralphy/runstate/` is written OR removed.
//! Mirrors `tests/tree_watch.rs`'s `serve_repo` harness; the distinguishing fact
//! this pins over the wire is the gitignore exemption — the repo root carries a
//! `.gitignore` listing `.ralphy/`, which the pump's filter would otherwise drop.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const RUNID: &str = "01TESTRUNIDTESTRUNIDTE";

/// Bind a daemon over a temp repo whose `.gitignore` hides `.ralphy/`; return the
/// `ws://…/ws/tree` URL, the repo slug, and the repo root.
async fn serve_repo() -> (String, String, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    // The trap the exemption exists for: every repo ralphy touches gitignores
    // `.ralphy/`, so without the exemption no snapshot write ever nudges.
    std::fs::write(root.join(".gitignore"), b".ralphy/\n").unwrap();

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

/// Send a subscription command for `repo` over the open socket. The `runs.*`
/// verbs ignore `path`; it is sent anyway to prove they do.
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

/// Poll `cond` until it holds or 10s pass. Returns whether it held.
async fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Wait up to 10s for a frame with `verb` and return its `repo` payload field.
async fn recv_verb(ws: &mut Ws, verb: &str) -> Option<String> {
    recv_verb_within(ws, verb, Duration::from_secs(10)).await
}

/// The bounded form — used for the NEGATIVE assertion, where a 10s wait would
/// only make the suite slow.
async fn recv_verb_within(ws: &mut Ws, verb: &str, dur: Duration) -> Option<String> {
    tokio::time::timeout(dur, async {
        while let Some(msg) = ws.next().await {
            let bytes = match msg {
                Ok(Message::Binary(b)) => b,
                Ok(Message::Close(_)) | Err(_) => return None,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.verb == verb {
                    return Some(cmd.payload["repo"].as_str().unwrap_or("").to_string());
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// A minimal `v:1` snapshot document. Its pid is THIS process, so the reader
/// would see a live run — the watcher never parses it, but a plausible document
/// keeps the fixture honest against `runs.list`.
fn document() -> String {
    serde_json::json!({
        "v": 1,
        "runid": RUNID,
        "pid": std::process::id(),
        "title": "the #300 watch fixture",
        "repo": "owner/tree",
        "branch": "afk/run-300",
        "plan_agent": "claude",
        "exec_agent": "claude",
        "started_at": "2026-07-25T10:00:00-03:00",
        "plan_path": ".ralphy/plan.md",
        "queue": { "total": 1, "order": [300], "stop_before": null },
        "issues": [{ "number": 300, "title": "live runs", "status": "executing", "blocked_by": [] }],
        "phase": { "active": 300, "state": "executing", "sleep": null, "final_summary": null },
    })
    .to_string()
}

#[tokio::test]
async fn runstate_write_pushes_runs_dirty() {
    let (url, slug, root) = serve_repo().await;
    let runstate = root.join(".ralphy").join("runstate");

    let (mut ws, _resp) = connect_async(&url).await.expect("connect /ws/tree");
    send_verb(&mut ws, "runs.watch", &slug, "ignored/path").await;
    // Poll rather than sleep-then-assert: a fixed nap used as synchronization
    // fails a conformant daemon on a loaded host.
    assert!(
        wait_until(|| runstate.is_dir()).await,
        "runs.watch creates the snapshot dir so a first run is never invisible"
    );
    // The dir now exists; give the OS watch a beat to attach before the write.
    tokio::time::sleep(Duration::from_millis(300)).await;

    std::fs::write(runstate.join(format!("{RUNID}.json")), document()).unwrap();

    assert_eq!(
        recv_verb(&mut ws, "runs.dirty").await,
        Some(slug.clone()),
        "a snapshot write pushes runs.dirty despite `.ralphy/` being gitignored"
    );
}

/// `runs.unwatch` really releases: after it, the same connection receives
/// nothing more — even though `runs.watch` was sent TWICE, so the duplicate
/// took no second hold that one release would fail to undo.
#[tokio::test]
async fn runs_unwatch_stops_the_pushes() {
    let (url, slug, root) = serve_repo().await;
    let runstate = root.join(".ralphy").join("runstate");

    let (mut ws, _resp) = connect_async(&url).await.expect("connect /ws/tree");
    send_verb(&mut ws, "runs.watch", &slug, "").await;
    send_verb(&mut ws, "runs.watch", &slug, "some/other/path").await;
    assert!(wait_until(|| runstate.is_dir()).await, "the watch is up");
    tokio::time::sleep(Duration::from_millis(300)).await;

    std::fs::write(runstate.join(format!("{RUNID}.json")), document()).unwrap();
    assert_eq!(
        recv_verb(&mut ws, "runs.dirty").await,
        Some(slug.clone()),
        "the subscription is live before the release"
    );

    send_verb(&mut ws, "runs.unwatch", &slug, "").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    std::fs::write(runstate.join("01OTHERRUNIDOTHERRUNID.json"), document()).unwrap();
    assert_eq!(
        recv_verb_within(&mut ws, "runs.dirty", Duration::from_secs(3)).await,
        None,
        "a released subscription pushes nothing further"
    );
}

#[tokio::test]
async fn snapshot_removal_pushes_runs_dirty() {
    let (url, slug, root) = serve_repo().await;
    let runstate = root.join(".ralphy").join("runstate");
    std::fs::create_dir_all(&runstate).unwrap();
    let doc = runstate.join(format!("{RUNID}.json"));
    std::fs::write(&doc, document()).unwrap();

    let (mut ws, _resp) = connect_async(&url).await.expect("connect /ws/tree");
    send_verb(&mut ws, "runs.watch", &slug, "").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // A run that ends removes its document (the panel must empty without a reload).
    std::fs::remove_file(&doc).unwrap();

    assert_eq!(
        recv_verb(&mut ws, "runs.dirty").await,
        Some(slug.clone()),
        "a finished run's document removal pushes runs.dirty"
    );
}
