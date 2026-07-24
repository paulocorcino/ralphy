//! Teardown invariant over a real loopback WebSocket (docs/adr/0032 §2; PRD #157
//! stories 18/20; issue #180): a client fires a `run`, reads the `spawned` ack,
//! then DROPS the `/ws/command` socket immediately — and the dispatched child
//! still runs to completion. Proven machine-side: the child writes a
//! `dispatch-done` sentinel (via `RALPHY_TEST_DONE_FILE`) AFTER a delay, and this
//! test observes the sentinel appear despite the disconnect. No client kill.
//!
//! Own file (single test): `RALPHY_EXE_OVERRIDE` and friends are process-global,
//! so keeping this the binary's sole test avoids the intra-process env race.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

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
async fn dispatched_run_survives_a_client_disconnect_after_the_ack() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/teardown";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    let done_file = dir.path().join("done");
    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "0");
    // Delay the child's exit so the client can disconnect while it is still
    // running; the child writes the sentinel only just before it exits.
    std::env::set_var("RALPHY_TEST_SLEEP_MS", "300");
    std::env::set_var("RALPHY_TEST_DONE_FILE", &done_file);

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

    // Read only up to the `spawned` ack, then drop the socket mid-run.
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(msg) = ws.next().await {
            let Ok(Message::Binary(bytes)) = msg else {
                continue;
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.payload.get("status").and_then(|s| s.as_str()) == Some("spawned") {
                    return;
                }
            }
        }
        panic!("never received the spawn ack");
    })
    .await
    .expect("the spawn ack must arrive within 10s");

    // DROP the client socket immediately after the ack — the invariant under test.
    drop(ws);

    // The child (sleeping 300ms) must still run to completion and write the
    // sentinel. Poll up to 5s.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(contents) = std::fs::read_to_string(&done_file) {
            assert_eq!(
                contents, "dispatch-done",
                "the child must complete and write the sentinel despite the disconnect"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the sentinel {} never appeared — the run did not survive the client disconnect",
            done_file.display()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
