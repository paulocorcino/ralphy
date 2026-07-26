//! The three working-tree write verbs reach the child and answer on the
//! requesting id (issue #318; ADR-0036 §2/§6). Each spawns-and-collects
//! `command_test_child` (pointed at via `RALPHY_EXE_OVERRIDE`), which echoes its
//! argv; a non-zero exit relays as `status:"error"` with that echo as the
//! message — the shape a run-lock refusal takes on the wire.
//!
//! The legs assert each verb's OWN argv and the ABSENCE of a sibling's, so a
//! builder that answered `stage` for all three would still red. The last leg is
//! the no-argv path: a malformed payload must be refused by the daemon, with no
//! child spawned at all.
//!
//! SOLE env-setter in its file (see `command_config.rs`): `RALPHY_EXE_OVERRIDE`
//! and `RALPHY_TEST_*` are process-global. The legs run SEQUENTIALLY inside one
//! test so nothing races on them.

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

/// The error message of a Mutate reply that relayed a non-zero exit.
fn relayed(reply: &serde_json::Value) -> String {
    assert_eq!(
        reply["status"], "error",
        "a non-zero exit relays as error; got {reply}"
    );
    reply["message"]
        .as_str()
        .unwrap_or_else(|| panic!("an error message string; got {reply}"))
        .to_string()
}

#[tokio::test]
async fn the_three_write_verbs_carry_their_own_argv_to_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/worktree";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    // Every leg relays a refusal, which is what a held run lock looks like here.
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "1");

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

    let staged = ask(
        port,
        1,
        "changes.stage",
        serde_json::json!({ "repo": slug, "paths": ["a.txt"] }),
    )
    .await;
    let msg = relayed(&staged);
    assert!(
        msg.contains("changes stage --path=a.txt"),
        "the stage argv must reach the child; got: {msg:?}"
    );
    assert!(
        !msg.contains("unstage") && !msg.contains("commit"),
        "no sibling's argv leaked into the stage leg: {msg:?}"
    );
    assert!(
        staged.get("changes").is_none(),
        "a Mutate reply carries no read field: {staged}"
    );

    let unstaged = ask(
        port,
        2,
        "changes.unstage",
        serde_json::json!({ "repo": slug, "paths": ["a.txt"] }),
    )
    .await;
    let msg = relayed(&unstaged);
    assert!(
        msg.contains("changes unstage --path=a.txt"),
        "the unstage argv must reach the child; got: {msg:?}"
    );
    assert!(
        !msg.contains("commit"),
        "no sibling's argv leaked into the unstage leg: {msg:?}"
    );

    let committed = ask(
        port,
        3,
        "changes.commit",
        serde_json::json!({ "repo": slug, "message": "hello" }),
    )
    .await;
    let msg = relayed(&committed);
    assert!(
        msg.contains("changes commit --message=hello"),
        "the commit argv must reach the child; got: {msg:?}"
    );
    assert!(
        !msg.contains("--path="),
        "the commit leg carries no path token: {msg:?}"
    );

    // The no-argv path: a malformed payload is refused BY THE DAEMON, so the
    // reply is its own fixed prose and no child echo appears in it.
    let refused = ask(
        port,
        4,
        "changes.stage",
        serde_json::json!({ "repo": slug, "paths": ["/etc/passwd"] }),
    )
    .await;
    assert_eq!(refused["status"], "error", "got {refused}");
    assert_eq!(
        refused["message"], "invalid mutation options",
        "a malformed path never reaches a child: {refused}"
    );

    let no_message = ask(
        port,
        5,
        "changes.commit",
        serde_json::json!({ "repo": slug }),
    )
    .await;
    assert_eq!(
        no_message["message"], "invalid mutation options",
        "an absent message never reaches a child: {no_message}"
    );
}
