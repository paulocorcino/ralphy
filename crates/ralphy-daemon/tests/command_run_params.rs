//! The daemon-composed run argv reaches the child (issue #191; ADR-0036 §1): a
//! `run` command carrying the modal's closed-enum params (`agent`, `branchMode`)
//! spawns `command_test_child`, which echoes its argv — proving `spawn_argv` →
//! CLI end to end. The streamed `status:"output"` frame must carry `--agent
//! claude` AND `--branch-mode new`, then `status:"exited"`. Mirrors
//! `tests/command_ws.rs`.
//!
//! SOLE env-setter in its file: `RALPHY_EXE_OVERRIDE`/`RALPHY_TEST_*` are
//! process-global, so an env-setting integration test must be alone in its file
//! (no intra-process race).

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn run_command_argv_reaches_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/params";
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
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
        std::path::PathBuf::from("does-not-exist"),
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

    // The modal's chosen params ride the payload; the daemon composes the argv.
    ws.send(Message::Binary(protocol::encode(&Frame::Command(
        Command {
            id: 1,
            verb: "run".to_string(),
            payload: serde_json::json!({ "repo": slug, "agent": "claude", "branchMode": "new" }),
        },
    ))))
    .await
    .unwrap();

    let (output, exited) = tokio::time::timeout(Duration::from_secs(10), async {
        let mut exited: Option<serde_json::Value> = None;
        let mut output = String::new();
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                match cmd.payload.get("status").and_then(|s| s.as_str()) {
                    Some("output") => {
                        output.push_str(cmd.payload["chunk"].as_str().unwrap_or_default());
                    }
                    Some("exited") => {
                        exited = Some(cmd.payload);
                        break;
                    }
                    _ => {}
                }
            }
        }
        (output, exited)
    })
    .await
    .expect("output + exit must arrive within 10s");

    // The composed flags reached the child (echoed by `command_test_child`).
    assert!(
        output.contains("--agent claude"),
        "the run argv must carry --agent claude; got: {output:?}"
    );
    assert!(
        output.contains("--branch-mode new"),
        "the run argv must carry --branch-mode new; got: {output:?}"
    );

    let exited = exited.expect("must receive an exit frame");
    assert_eq!(exited["status"], "exited");
    assert_eq!(exited["code"].as_i64(), Some(0));
}
