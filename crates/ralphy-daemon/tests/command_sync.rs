//! The sync verbs reach the child and answer on the requesting id (issue #316;
//! ADR-0036 §2/§6): `sync.status` spawns-and-COLLECTS `command_test_child`
//! (pointed at via `RALPHY_EXE_OVERRIDE`), which echoes its argv — proving
//! `sync_status_argv` → CLI end to end, under the `sync` field the browser folds
//! from (`reply.sync.sync`); `sync.fetch` exits non-zero and must relay as
//! `status:"error"` with the child's output as the message, which is the shape a
//! run-lock refusal takes.
//!
//! SOLE env-setter in its file (see `command_config.rs`): `RALPHY_EXE_OVERRIDE`
//! and `RALPHY_TEST_*` are process-global. The two legs run SEQUENTIALLY inside
//! one test so the exit code can differ per leg without an intra-process race.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

/// One command round-trip on its own socket, returning the reply payload.
async fn ask(port: u16, id: u64, verb: &str, payload: serde_json::Value) -> serde_json::Value {
    let url = format!("ws://127.0.0.1:{port}/ws/command");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
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
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                Message::Close(_) => break,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.id == id {
                    return Some(cmd.payload);
                }
            }
        }
        None
    })
    .await
    .expect("a reply must arrive within 10s")
    .expect("a reply on the requesting id")
}

#[tokio::test]
async fn sync_status_and_fetch_argv_reach_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/sync";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "0");

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
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let status = ask(port, 1, "sync.status", serde_json::json!({ "repo": slug })).await;
    assert_eq!(status["status"], "ok", "sync query is ok; got {status}");
    // The child's echo is not JSON, so `sync` is the raw collected stdout — it
    // must carry the composed argv AND ride the `sync` field the browser folds.
    let raw = status["sync"]
        .as_str()
        .unwrap_or_else(|| panic!("the reply must carry a `sync` field; got {status}"));
    assert!(
        raw.contains("sync status --format json"),
        "the sync-status argv must reach the child; got: {raw:?}"
    );

    // Second leg: a non-zero exit is how a run-lock refusal reaches the UI, and
    // the exit code is re-set between legs (the child reads it at spawn).
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "1");
    let fetched = ask(port, 2, "sync.fetch", serde_json::json!({ "repo": slug })).await;
    assert_eq!(
        fetched["status"], "error",
        "a non-zero exit relays as error; got {fetched}"
    );
    let message = fetched["message"]
        .as_str()
        .unwrap_or_else(|| panic!("an error message string; got {fetched}"));
    assert!(
        message.contains("sync fetch"),
        "the sync-fetch argv must reach the child; got: {message:?}"
    );
    assert!(
        fetched.get("sync").is_none(),
        "a Mutate reply carries no read field: {fetched}"
    );
}
