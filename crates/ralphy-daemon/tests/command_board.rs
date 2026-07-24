//! The board Query verb reaches the child and answers on the requesting id
//! (issue #198; ADR-0036 §2, slice 6): a `board.list` for a registered repo
//! spawns-and-COLLECTS `command_test_child` (pointed at via `RALPHY_EXE_OVERRIDE`),
//! which echoes its argv — proving `board_argv` → CLI end to end. The reply carries
//! `status:"ok"`; because the child's echo is not valid JSON, the `board` field is
//! the raw collected stdout, which must contain `issues --format json --board`.
//!
//! SOLE env-setter in its file: `RALPHY_EXE_OVERRIDE`/`RALPHY_TEST_*` are
//! process-global, so an env-setting integration test must be alone in its file.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn board_list_argv_reaches_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/board";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    // Sole test in this file → no intra-process env race.
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

    let url = format!("ws://127.0.0.1:{port}/ws/command");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting to /ws/command");

    ws.send(Message::Binary(protocol::encode(&Frame::Command(
        Command {
            id: 1,
            verb: "board.list".to_string(),
            payload: serde_json::json!({ "repo": slug }),
        },
    ))))
    .await
    .unwrap();

    let reply = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                Message::Close(_) => break,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.id == 1 {
                    return Some(cmd.payload);
                }
            }
        }
        None
    })
    .await
    .expect("a reply must arrive within 10s")
    .expect("a reply on id 1");

    assert_eq!(reply["status"], "ok", "board query is ok; got {reply}");
    // The child's echo is not JSON, so `board` is the raw collected stdout — it
    // must carry the composed argv (proving `board_argv` reached the child).
    let raw = reply["board"].as_str().expect("raw board string");
    assert!(
        raw.contains("issues --format json --board"),
        "the board argv must reach the child; got: {raw:?}"
    );
}
