//! ADR-0042 D6 over the workbench's interactive launch (issue #248): the indexing
//! gate is a product stance, not a run-path implementation detail, so opening a
//! Cursor console from the UI must refuse an unprotected repository exactly the way
//! `ralphy run --agent cursor` does — and refuse it BEFORE anything is spawned.
//!
//! Two legs against one live loopback daemon: the same URL is refused with `400`
//! and no session, then accepted once `.cursorindexingignore` exists, streaming
//! through the codec + PTY like any other vendor.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

fn terminal(data: &[u8]) -> Message {
    Message::Binary(protocol::encode(&Frame::Terminal {
        session: 1,
        data: data.to_vec(),
    }))
}

/// A raw HTTP/1.1 GET on the live listener, returning the body. Raw sockets rather
/// than `oneshot` because the assertion is about the SERVING router's own session
/// state — a second `router()` would have its own empty session manager and the
/// "nothing was spawned" claim would be vacuous.
async fn http_get(port: u16, path: &str) -> String {
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    sock.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut raw = String::new();
    sock.read_to_string(&mut raw).await.unwrap();
    raw.split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or(raw)
}

#[tokio::test]
async fn cursor_session_refuses_an_unprotected_repo_and_spawns_a_protected_one() {
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

    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fcursorlab&agent=cursor");

    // --- Leg 1: no opt-out file → the upgrade is refused and nothing is spawned.
    let err = tokio_tungstenite::connect_async(&url)
        .await
        .expect_err("an unprotected repo must NOT upgrade");
    let (status, body) = match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            let status = resp.status();
            let body = String::from_utf8_lossy(resp.body().as_deref().unwrap_or(&[])).into_owned();
            (status, body)
        }
        other => panic!("expected an HTTP refusal, got {other:?}"),
    };
    assert_eq!(status.as_u16(), 400, "the refusal must be a 400");
    assert!(
        body.contains(".cursorindexingignore"),
        "the refusal must name the opt-out file the operator has to create; got:\n{body}"
    );
    assert_eq!(
        http_get(port, "/api/sessions").await,
        "[]",
        "the refusal must return BEFORE spawn_attached — no child, no session record"
    );

    // --- Leg 2: opted out → the same URL launches and streams.
    std::fs::write(dir.path().join(".cursorindexingignore"), "*\n").unwrap();
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("a protected repo must upgrade");

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

    // --- Leg 3: no opt-out file, but the operator opted IN through the settings
    // file the daemon reparses. Proves the route reads the opt-in from the
    // REGISTERED repo's directory (not the cwd, not a default), and that the gate's
    // escape hatch is reachable from the workbench and not only from the run path.
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

    // And flipping it back to `false` restores the refusal — so leg 3 proved the
    // opt-in, not merely that the gate stopped firing for some other reason.
    std::fs::write(
        &settings,
        r#"{"cursor":{"allow_codebase_indexing_i_understand_the_risk":false}}"#,
    )
    .unwrap();
    let err = tokio_tungstenite::connect_async(&url)
        .await
        .expect_err("an explicit opt-OUT must refuse again");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status().as_u16(), 400)
        }
        other => panic!("expected an HTTP refusal, got {other:?}"),
    }
}
