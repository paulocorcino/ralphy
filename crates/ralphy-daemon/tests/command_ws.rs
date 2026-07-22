//! End-to-end remote command over a real loopback WebSocket (docs/adr/0032 §2;
//! issue #163): a client connects to `/ws/command`, sends a `run` command for a
//! registered slug (the spawned exe overridden to `command_test_child`, which
//! exits 7), and receives the spawn ack (`status:"spawned"` + a pid) followed by
//! the child's exit frame (`status:"exited"`, `code:7`) — proving the WS →
//! dispatch → spawn → ack/exit path. Mirrors `tests/session_ws.rs`.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

/// Encode the command frame the browser button would send.
fn command(id: u64, verb: &str, slug: &str) -> Message {
    Message::Binary(protocol::encode(&Frame::Command(Command {
        id,
        verb: verb.to_string(),
        // `run` now requires validated closed-enum params (#191); triage/push
        // ignore them.
        payload: serde_json::json!({ "repo": slug, "agent": "claude", "branchMode": "new" }),
    })))
}

#[tokio::test]
async fn command_ws_spawns_a_run_and_reports_ack_then_exit() {
    // A registry with one reachable slug pointing at a temp dir.
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/dispatch";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    // Point the dispatcher at the helper bin (exit code 7) instead of `ralphy`.
    // This is the only test that sets these, its sole test → no intra-process env
    // race.
    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "7");

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

    ws.send(command(1, "run", slug)).await.unwrap();

    let (spawned, output, exited) = tokio::time::timeout(Duration::from_secs(10), async {
        let mut spawned: Option<serde_json::Value> = None;
        let mut exited: Option<serde_json::Value> = None;
        // Concatenated `chunk` text from every `status:"output"` frame, in order.
        let mut output = String::new();
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                match cmd.payload.get("status").and_then(|s| s.as_str()) {
                    Some("spawned") => spawned = Some(cmd.payload),
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
        (spawned, output, exited)
    })
    .await
    .expect("ack + output + exit must arrive within 10s");

    let spawned = spawned.expect("must receive a spawn ack frame");
    assert_eq!(spawned["status"], "spawned");
    assert!(
        spawned["pid"].as_u64().is_some(),
        "the ack must carry a numeric pid; got: {spawned}"
    );

    // Output arrives BEFORE the exit frame (we break on exit, so anything the
    // loop accumulated in `output` was received earlier) and carries BOTH the
    // child's stdout and stderr markers — proving the merged-pipe stream works.
    assert!(
        output.contains("dispatch-stdout-marker"),
        "the streamed output must carry the stdout marker; got: {output:?}"
    );
    assert!(
        output.contains("dispatch-stderr-marker"),
        "the streamed output must carry the stderr marker; got: {output:?}"
    );

    let exited = exited.expect("must receive an exit frame");
    assert_eq!(exited["status"], "exited");
    assert_eq!(
        exited["code"].as_i64(),
        Some(7),
        "the exit frame must carry the child's code 7; got: {exited}"
    );
}
