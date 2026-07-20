//! The config Mutate verb reaches the child and relays a non-zero exit as an
//! error (issue #195; ADR-0036 §2/§6): a `config.set` for a registered repo
//! spawns-and-collects `command_test_child`, which echoes its argv and exits
//! non-zero — proving BOTH that `config_argv` composed `config set branch_mode
//! new` end to end AND that the Mutate branch relays a non-zero exit (the shape a
//! run-lock refusal or unknown-key error takes) as `status:"error"` with the
//! child's output as the message.
//!
//! SOLE env-setter in its file (see `command_config.rs`).

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn config_set_argv_reaches_the_child_and_nonzero_relays() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/cfgset";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    // Non-zero so the Mutate branch relays the child's output (which carries the
    // echoed argv) as an error message.
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "1");

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
            id: 2,
            verb: "config.set".to_string(),
            payload: serde_json::json!({ "repo": slug, "key": "branch_mode", "value": "new" }),
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
                if cmd.id == 2 {
                    return Some(cmd.payload);
                }
            }
        }
        None
    })
    .await
    .expect("a reply must arrive within 10s")
    .expect("a reply on id 2");

    assert!(
        reply.get("status").is_some(),
        "a status is present: {reply}"
    );
    assert_eq!(reply["status"], "error", "a non-zero exit relays as error");
    let msg = reply["message"].as_str().expect("an error message string");
    assert!(
        msg.contains("config set -- branch_mode new"),
        "the config set argv must reach the child; got: {msg:?}"
    );
}
