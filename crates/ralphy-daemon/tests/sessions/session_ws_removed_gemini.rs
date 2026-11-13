//! The Gemini CLI agent is gone. A desk saved before its removal still names a
//! `gemini` console; reopening it is refused with the message the CLI prints
//! for `--agent gemini`, before anything is spawned.

use std::time::Instant;

use ralphy_daemon::{registry, router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::page;

/// The body of `GET /api/sessions` on the live daemon: the sessions it holds.
async fn sessions(port: u16) -> String {
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the daemon");
    let request = "GET /api/sessions HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    sock.write_all(request.as_bytes())
        .await
        .expect("send the request");
    let mut raw = String::new();
    sock.read_to_string(&mut raw)
        .await
        .expect("read the response");
    raw.split_once("\r\n\r\n")
        .map_or(raw.clone(), |(_, body)| body.to_owned())
}

#[tokio::test]
async fn a_saved_gemini_console_is_refused_with_the_removed_message() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/oldlab", &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).expect("save the registry");
    super::point_launcher_at_test_child();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let port = listener.local_addr().expect("the bound address").port();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        dir.path().join("no-ui"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        let _keep_open = tx;
        axum::serve(listener, app).await.expect("serve the daemon");
    });

    let launch = serde_json::json!({ "repo": "owner/oldlab", "agent": "gemini" });
    let refusal = page::refused(port, launch).await;

    assert_eq!(
        refusal["message"], "the Gemini CLI agent was removed: use --agent antigravity",
        "{refusal}"
    );
    assert_eq!(
        sessions(port).await,
        "[]",
        "a refused launch starts no console"
    );
    // The browser's fold reads this very file (`endNotice`).
    super::golden::check(
        "session-end--refused",
        refusal,
        &["/daemon_id", "/environment"],
    );
}
