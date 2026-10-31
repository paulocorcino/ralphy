//! The refusals of a `console` channel that come before anything is spawned:
//! a new launch is refused by a `session-end` frame with the reason as its
//! message, and a reattach of an unknown session by `channel.closed` with
//! reason `refused` and no frame.

use std::path::Path;
use std::time::{Duration, Instant};

use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

use serde_json::json;

use crate::page;

/// A live loopback daemon whose registry is `repos.toml` in `dir`.
async fn serve(dir: &Path) -> u16 {
    super::point_launcher_at_test_child();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        dir.join("repos.toml"),
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        let _keep_open = tx;
        axum::serve(listener, app)
            .await
            .expect("serve the test app");
    });
    port
}

async fn refusal(port: u16, fields: serde_json::Value) -> String {
    let payload = page::refused(port, fields).await;
    payload["message"]
        .as_str()
        .expect("read the message field")
        .to_string()
}

fn register(dir: &Path, slug: &str) {
    let mut store = registry::RegistryStore::default();
    store.upsert(slug, &dir.to_string_lossy());
    registry::save_to(&store, &dir.join("repos.toml")).expect("save the registry");
}

#[tokio::test]
async fn an_agent_launch_names_a_known_agent_and_a_registered_repo() {
    let dir = tempfile::tempdir().unwrap();
    register(dir.path(), "owner/known");
    let port = serve(dir.path()).await;

    assert_eq!(
        refusal(port, json!({ "repo": "owner/known" })).await,
        "unknown agent",
        "no agent"
    );
    assert_eq!(
        refusal(port, json!({ "repo": "owner/known", "agent": "nobody" })).await,
        "unknown agent",
        "an agent no vendor answers to"
    );
    assert_eq!(
        refusal(port, json!({ "agent": "claude" })).await,
        "unknown repo",
        "no repo"
    );
    assert_eq!(
        refusal(port, json!({ "repo": "owner/other", "agent": "claude" })).await,
        "unknown repo",
        "a repo nobody registered"
    );
}

#[tokio::test]
async fn a_console_launch_needs_a_registered_repo_when_it_names_one() {
    let dir = tempfile::tempdir().unwrap();
    register(dir.path(), "owner/known");
    let port = serve(dir.path()).await;

    assert_eq!(
        refusal(port, json!({ "console": 1, "repo": "owner/other" })).await,
        "unknown repo"
    );
}

#[tokio::test]
async fn an_unreadable_registry_refuses_every_launch() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("repos.toml"), "this = = is not toml").unwrap();
    let port = serve(dir.path()).await;

    assert_eq!(
        refusal(port, json!({ "repo": "owner/known", "agent": "claude" })).await,
        "repo registry unreadable",
        "an agent launch"
    );
    assert_eq!(
        refusal(port, json!({ "console": 1, "repo": "owner/known" })).await,
        "repo registry unreadable",
        "a console launch"
    );
}

/// A reattach of an unknown session is refused before anything is sent: no
/// frame, and no `session-open`.
#[tokio::test]
async fn a_reattach_of_an_unknown_session_is_refused_with_no_frame() {
    let dir = tempfile::tempdir().unwrap();
    let port = serve(dir.path()).await;

    let mut channel = page::console(port, json!({ "id": 9999 })).await;
    let mut frames = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = channel.next().await {
            match message.expect("the page socket reads") {
                Message::Binary(bytes) => frames.push(bytes.to_vec()),
                Message::Close(_) => break,
                Message::Text(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    })
    .await
    .expect("the daemon ends the reattach within 10s");
    assert!(
        frames.is_empty(),
        "a refused reattach sends no frame: {frames:?}"
    );
    assert_eq!(channel.closed_reason(), Some("refused"));
}
