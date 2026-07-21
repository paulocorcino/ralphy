//! Free-console session over a real loopback WebSocket (issue #167): a client
//! connects to `/ws/session?console=1&repo=<slug>` (the console's program
//! overridden to the helper bin), types a line, and reads it echoed back
//! through the codec + PTY — proving the WS → codec → PTY → child path AND
//! that the child was spawned in the CHOSEN repo's directory (the helper's
//! startup `CWD:` line). Then it asserts `GET /api/sessions` lists the session
//! with `kind: "console"` (distinct from the curated agent path's `"agent"`,
//! pinned by `session_persistence.rs:157`).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

/// Encode a terminal keystroke frame the way the browser would.
fn terminal(data: &[u8]) -> Message {
    Message::Binary(protocol::encode(&Frame::Terminal {
        session: 1,
        data: data.to_vec(),
    }))
}

/// A raw HTTP/1.1 request over a fresh `TcpStream` against the live
/// `axum::serve` listener. Copied from `session_persistence.rs`'s
/// `http_request` helper (no general HTTP client among the crate's dev-deps).
async fn http_request(port: u16, method: &str, path: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

#[tokio::test]
async fn console_ws_spawns_shell_in_chosen_repo_and_lists_as_console_kind() {
    // A registry with one reachable slug pointing at a temp dir — the
    // console's chosen cwd.
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/console-repo";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    // Point the console's launcher at the helper bin instead of a real shell.
    // This is this file's only such test → no intra-process env race
    // (`session_ws.rs` documents itself as its own file's sole setter).
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
        std::time::Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("ws://127.0.0.1:{port}/ws/session?console=1&repo=owner%2Fconsole-repo");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting to /ws/session");

    // Type a line and read until the child echoes it, along the way answering
    // the ConPTY startup cursor-position request (xterm.js's role in prod) and
    // capturing the startup CWD: line proving the chosen repo's dir was used.
    ws.send(terminal(b"hello-console\r")).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(10), async {
        let mut acc = String::new();
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                _ => continue,
            };
            if let Ok(Frame::Terminal { data, .. }) = protocol::decode(&bytes) {
                if data
                    .windows(CURSOR_POSITION_REQUEST.len())
                    .any(|w| w == CURSOR_POSITION_REQUEST)
                {
                    ws.send(terminal(CURSOR_POSITION_REPLY)).await.unwrap();
                }
                acc.push_str(&String::from_utf8_lossy(&data));
                if acc.contains("GOT:hello-console") {
                    return acc;
                }
            }
        }
        acc
    })
    .await
    .expect("keystroke round-trip must complete within 10s");

    let expected_cwd = format!("CWD:{}", dir.path().display());
    assert!(
        got.contains(&expected_cwd),
        "console child must spawn in the chosen repo's dir; expected {expected_cwd:?} in:\n{got}"
    );
    assert!(
        got.contains("GOT:hello-console"),
        "server must echo the typed line back through codec + PTY; got:\n{got}"
    );

    let (status, body) = http_request(port, "GET", "/api/sessions").await;
    assert_eq!(status, 200, "GET /api/sessions body:\n{body}");
    let list: serde_json::Value = serde_json::from_str(&body).expect("sessions JSON");
    let arr = list.as_array().expect("sessions is an array");
    assert_eq!(arr.len(), 1, "one live console session: {body}");
    let entry = &arr[0];
    assert_eq!(entry["kind"], "console", "listed session's kind is console");
    assert_eq!(
        entry["agent"], "console",
        "listed session's agent label is console"
    );
}
