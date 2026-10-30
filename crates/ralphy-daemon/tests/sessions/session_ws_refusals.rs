//! The refusals of `/ws/session` that come before anything is spawned: a new
//! launch is refused by a `session-end` frame with the reason as its message,
//! and a reattach of an unknown session by HTTP `404`.

use std::path::Path;
use std::time::Instant;

use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite;

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

async fn refusal(port: u16, query: &str) -> String {
    let payload = super::refused(&format!("ws://127.0.0.1:{port}/ws/session?{query}")).await;
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
        refusal(port, "repo=owner%2Fknown").await,
        "unknown agent",
        "no agent"
    );
    assert_eq!(
        refusal(port, "repo=owner%2Fknown&agent=nobody").await,
        "unknown agent",
        "an agent no vendor answers to"
    );
    assert_eq!(
        refusal(port, "agent=claude").await,
        "unknown repo",
        "no repo"
    );
    assert_eq!(
        refusal(port, "repo=owner%2Fother&agent=claude").await,
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
        refusal(port, "console=1&repo=owner%2Fother").await,
        "unknown repo"
    );
}

#[tokio::test]
async fn an_unreadable_registry_refuses_every_launch() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("repos.toml"), "this = = is not toml").unwrap();
    let port = serve(dir.path()).await;

    assert_eq!(
        refusal(port, "repo=owner%2Fknown&agent=claude").await,
        "repo registry unreadable",
        "an agent launch"
    );
    assert_eq!(
        refusal(port, "console=1&repo=owner%2Fknown").await,
        "repo registry unreadable",
        "a console launch"
    );
}

#[tokio::test]
async fn a_reattach_of_an_unknown_session_is_a_404_before_the_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let port = serve(dir.path()).await;

    let url = format!("ws://127.0.0.1:{port}/ws/session?id=9999");
    match tokio_tungstenite::connect_async(&url).await {
        Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status(), 404),
        other => panic!("an attach to an unknown id must be 404, got {other:?}"),
    }
}
