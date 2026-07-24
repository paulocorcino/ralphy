//! ADR-0042 D6 over the workbench's interactive launch (issue #248): the indexing
//! gate is a product stance, not a run-path implementation detail, so opening a
//! Cursor console from the UI protects an unprotected repository exactly the way
//! `ralphy run --agent cursor` does — writing `.cursorindexingignore` BEFORE
//! anything is spawned.
//!
//! Two legs against one live loopback daemon: an unprotected repo upgrades, and
//! the opt-out file exists on disk afterwards while the session streams through
//! the codec + PTY like any other vendor; then an explicit opt-in reaches the
//! capability while writing no file.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio_tungstenite::tungstenite::Message;

fn terminal(data: &[u8]) -> Message {
    Message::Binary(protocol::encode(&Frame::Terminal {
        session: 1,
        data: data.to_vec(),
    }))
}

#[tokio::test]
async fn cursor_session_protects_an_unprotected_repo_then_streams() {
    // A registered repo that LOOKS like a git checkout — the gate walks for `.git`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/cursorlab";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    std::env::set_var(
        "RALPHY_DAEMON_AGENT_OVERRIDE",
        env!("CARGO_BIN_EXE_session_test_child"),
    );

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

    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fcursorlab&agent=cursor");

    // --- Leg 1: no opt-out file → the daemon WRITES it before spawning, then the
    // same URL upgrades and streams. The gate no longer refuses; it protects.
    assert!(
        !dir.path().join(".cursorindexingignore").exists(),
        "precondition: the repo starts unprotected"
    );
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("an unprotected repo must upgrade once the gate has written the opt-out");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".cursorindexingignore")).unwrap(),
        "*\n",
        "the interactive launch must write the opt-out BEFORE spawning, exactly like the run path"
    );

    ws.send(terminal(b"hello-cursor\r")).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(10), async {
        let mut acc = String::new();
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                _ => continue,
            };
            if let Ok(Frame::Terminal { data, .. }) = protocol::decode(&bytes) {
                // Play the terminal emulator: answer ConPTY's startup `ESC[6n` so
                // the child unblocks on Windows.
                if data
                    .windows(CURSOR_POSITION_REQUEST.len())
                    .any(|w| w == CURSOR_POSITION_REQUEST)
                {
                    ws.send(terminal(CURSOR_POSITION_REPLY)).await.unwrap();
                }
                acc.push_str(&String::from_utf8_lossy(&data));
                if acc.contains("GOT:hello-cursor") {
                    return acc;
                }
            }
        }
        acc
    })
    .await
    .expect("the cursor session's keystroke round-trip must complete within 10s");
    assert!(
        got.contains("GOT:hello-cursor"),
        "a protected repo's cursor console must stream like any other vendor; got:\n{got}"
    );

    ws.send(terminal(b"quit\r")).await.unwrap();

    // --- Leg 2: the operator opted IN through the settings file the daemon
    // reparses. Proves the route reads the opt-in from the REGISTERED repo's
    // directory (not the cwd, not a default), and that opt-in reaches the
    // capability while writing NO opt-out — an operator who wants the indexing must
    // not find Ralphy's file suppressing it.
    std::fs::remove_file(dir.path().join(".cursorindexingignore")).unwrap();
    let settings = dir.path().join(".ralphy").join("settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        r#"{"cursor":{"allow_codebase_indexing_i_understand_the_risk":true}}"#,
    )
    .unwrap();
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("an explicit opt-in must reach the capability");
    ws.send(terminal(b"quit\r")).await.unwrap();
    assert!(
        !dir.path().join(".cursorindexingignore").exists(),
        "an opted-in run must NOT have the opt-out written under it — that would suppress \
         the very indexing the operator asked for"
    );
}
