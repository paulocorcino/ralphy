//! `runs.list` over the real wire protocol (issue #299; ADR-0047 §9): the
//! Observe branch answers on the requesting `Command` id with the repo's live
//! snapshot documents and NEVER spawns. A document whose pid is dead is swept,
//! not listed; a malformed one is reported under `unreadable`, not hidden.
//! Mirrors `tests/observe_read.rs`.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

const LIVE_RUNID: &str = "01TESTRUNIDTESTRUNIDTE";

/// Bind a daemon over a temp repo whose `.ralphy/runstate/` holds one live-pid
/// document, one dead-pid document and one malformed file. Returns the
/// `ws://…/ws/command` URL and the repo slug.
async fn serve_repo() -> (String, String) {
    let dir = tempfile::tempdir().unwrap();
    let runstate = dir.path().join(".ralphy").join("runstate");
    std::fs::create_dir_all(&runstate).unwrap();
    std::fs::write(
        runstate.join(format!("{LIVE_RUNID}.json")),
        serde_json::to_vec(&serde_json::json!({
            "v": 1,
            "runid": LIVE_RUNID,
            "pid": std::process::id(),
            "title": "runs.list over the wire",
            "started_at": "2026-07-24T10:00:00-03:00",
            "phase": { "state": "executing", "active": 71 },
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        runstate.join("01DEADRUNIDDEADRUNIDDE.json"),
        serde_json::to_vec(&serde_json::json!({
            "v": 1,
            "runid": "01DEADRUNIDDEADRUNIDDE",
            // Windows PIDs stay far below this; never a live process.
            "pid": 4_000_001u32,
            "started_at": "2026-07-24T09:00:00-03:00",
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(runstate.join("bad.json"), b"not json").unwrap();

    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/runs";
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
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
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
async fn runs_list_answers_on_id_without_spawn() {
    let (url, slug) = serve_repo().await;
    let (replies, spawned) =
        round_trip(&url, 1, "runs.list", serde_json::json!({ "repo": slug })).await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "an Observe read must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok");

    let runs = reply["runs"].as_array().expect("a runs array");
    assert_eq!(runs.len(), 1, "the dead-pid document is not a live run");
    assert_eq!(runs[0]["runid"], LIVE_RUNID);
    assert_eq!(runs[0]["phase"]["state"], "executing");
    assert_eq!(runs[0]["phase"]["active"], 71);

    let unreadable = reply["unreadable"].as_array().expect("an unreadable array");
    assert_eq!(unreadable.len(), 1, "unreadable={unreadable:?}");
    assert_eq!(unreadable[0]["runid"], "bad");
    assert_eq!(unreadable[0]["reason"], "malformed");
}
