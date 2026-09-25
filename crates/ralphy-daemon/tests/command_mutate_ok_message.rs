//! `worktree.add`'s CLEAN exit keeps what the child printed: the CLI reports
//! its carry-over warnings (`worktree.copy` / `worktree.share` entries it
//! skipped) on stdout AFTER the add succeeded, and the daemon used to discard
//! every byte of a zero exit. For this ONE verb the reply is
//! `{status:"ok", message:<trimmed output>}` when there is output; every
//! other Mutate verb keeps the bare `{status:"ok"}` (pinned by
//! `command_changes_mutate.rs`), and so does a silent add. Pinned here with
//! `command_test_child` exiting 0 and printing its markers.
//!
//! SOLE env-setter in its file (see `command_config.rs`).

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

async fn mutate_reply(
    port: u16,
    id: u64,
    verb: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    let url = format!("ws://127.0.0.1:{port}/ws/command");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting to /ws/command");

    ws.send(Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id,
            verb: verb.to_string(),
            payload,
        }))
        .into(),
    ))
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
async fn a_clean_mutate_exit_relays_the_childs_output_as_message() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/mutateok";
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

    let reply = mutate_reply(
        port,
        4,
        "worktree.add",
        serde_json::json!({ "repo": slug, "name": "wt-x", "base": "main" }),
    )
    .await;
    assert_eq!(reply["status"], "ok", "a zero exit is ok; got {reply}");
    let message = reply["message"]
        .as_str()
        .expect("a clean exit with output carries it as `message`");
    assert!(
        message.contains("dispatch-stdout-marker")
            && message.contains("worktree add --base=main -- wt-x"),
        "the child's own output, verbatim and trimmed; got: {message:?}"
    );
    assert_eq!(
        message,
        message.trim(),
        "trimmed like the error branch's message"
    );
}
