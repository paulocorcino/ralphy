//! The branch/label Mutate verbs reach the child and relay a non-zero exit as an
//! error (issue #199; ADR-0036 §2/§6): `branch.switch` and `label.set` for a
//! registered repo spawn-and-collect `command_test_child`, which echoes its argv
//! and exits non-zero — proving BOTH that `branch_argv`/`label_argv` composed the
//! blessed command line end to end AND that the Mutate branch relays a non-zero
//! exit (the shape a run-lock refusal or forge error takes) as `status:"error"`
//! with the child's output as the message.
//!
//! SOLE env-setter in its file (see `command_config.rs`).

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

async fn mutate_message(port: u16, id: u64, verb: &str, payload: serde_json::Value) -> String {
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

    let reply = tokio::time::timeout(Duration::from_secs(10), async {
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
    .expect("a reply on the requesting id");

    assert_eq!(reply["status"], "error", "a non-zero exit relays as error");
    reply["message"]
        .as_str()
        .expect("an error message string")
        .to_string()
}

#[tokio::test]
async fn branch_switch_and_label_set_argv_reach_the_child_and_nonzero_relays() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/mutategit";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    // Non-zero so the Mutate branch relays the child's output (the echoed argv).
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

    let switch_msg = mutate_message(
        port,
        2,
        "branch.switch",
        serde_json::json!({ "repo": slug, "name": "feat/x" }),
    )
    .await;
    assert!(
        switch_msg.contains("branch switch -- feat/x"),
        "the branch switch argv must reach the child; got: {switch_msg:?}"
    );

    let label_msg = mutate_message(
        port,
        3,
        "label.set",
        serde_json::json!({ "repo": slug, "number": 7, "label": "AFK", "op": "add" }),
    )
    .await;
    assert!(
        label_msg.contains("label set 7 --add=AFK"),
        "the label set argv must reach the child; got: {label_msg:?}"
    );
}
