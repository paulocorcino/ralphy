//! A malformed run is refused, not spawned (issue #191; ADR-0036 §1): a `run`
//! command with an out-of-enum `agent` yields exactly one `status:"error"` frame
//! and NO `status:"spawned"` — the closed-enum validation of `spawn_argv` fails
//! before any child is launched. Sets NO env and never spawns, so it is race-free
//! against the env-setting suites.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn malformed_run_is_refused_without_spawning() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/refusal";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

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

    // `agent:"bogus"` is out of the closed enum → refusal, no spawn.
    ws.send(Message::Binary(protocol::encode(&Frame::Command(
        Command {
            id: 1,
            verb: "run".to_string(),
            payload: serde_json::json!({ "repo": slug, "agent": "bogus", "branchMode": "new" }),
        },
    ))))
    .await
    .unwrap();

    let (errors, spawned) = tokio::time::timeout(Duration::from_secs(10), async {
        let mut errors = 0usize;
        let mut spawned = 0usize;
        // The handler sends the refusal then returns (dropping the socket); read
        // until the server closes or the stream ends.
        while let Some(msg) = ws.next().await {
            let bytes = match msg {
                Ok(Message::Binary(b)) => b,
                Ok(Message::Close(_)) | Err(_) => break,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                match cmd.payload.get("status").and_then(|s| s.as_str()) {
                    Some("error") => errors += 1,
                    Some("spawned") => spawned += 1,
                    _ => {}
                }
            }
        }
        (errors, spawned)
    })
    .await
    .expect("the refusal must arrive and the socket close within 10s");

    assert_eq!(errors, 1, "exactly one error frame");
    assert_eq!(spawned, 0, "a malformed run must never spawn");
}
