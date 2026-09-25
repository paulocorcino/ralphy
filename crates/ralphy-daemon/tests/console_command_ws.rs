//! The startup-command console over a real loopback WebSocket: a client
//! connects to `/ws/session?console=1&repo=<slug>&command=<cmd>` (the
//! console's program overridden to the helper bin), asks the child for its
//! argv, and reads back the `-lc <cmd>` the daemon composed — the command as
//! ONE argv element, never re-split. Then `GET /api/sessions` must label the
//! session's `agent` with the command (its `kind` stays `console`), which is
//! what lets the workbench relaunch it as the same console (`console_ws.rs`
//! pins the bare shell's `agent: "console"`).
//!
//! Its own file: the `RALPHY_DAEMON_AGENT_OVERRIDE` seam is process-wide.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

/// Encode a terminal keystroke frame the way the browser would.
fn terminal(data: &[u8]) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Terminal {
            session: 1,
            data: data.to_vec(),
        })
        .into(),
    )
}

/// A raw HTTP/1.1 request over a fresh `TcpStream` against the live
/// `axum::serve` listener. Copied from `session_persistence.rs`'s
/// `http_request` helper (no general HTTP client among the crate's dev-deps).
async fn http_request(port: u16, method: &str, path: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
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
async fn console_ws_runs_the_startup_command_and_labels_the_session_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/console-repo";
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
        std::time::Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // `htop -d 5` — a command WITH a space, so a re-split would show as two
    // argv elements. Percent-encoded as the browser's `encodeURIComponent` does.
    let url = format!(
        "ws://127.0.0.1:{port}/ws/session?console=1&repo=owner%2Fconsole-repo&command=htop%20-d%205"
    );
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting to /ws/session");

    // The helper's `argv` line prints its own argv (everything after the exe),
    // so the daemon's shell-command composition is observable end to end.
    ws.send(terminal(b"argv\r")).await.unwrap();
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
                if acc.contains("ARGV:") && acc.contains("htop -d 5") {
                    return acc;
                }
            }
        }
        acc
    })
    .await
    .expect("argv round-trip must complete within 10s");

    // The override is not pwsh/cmd by stem, so it takes the POSIX form — the
    // same branch a Linux/WSL `$SHELL` takes. The child joins argv with single
    // spaces, so `-lc` followed by the command as one element reads like this.
    assert!(
        got.contains("ARGV:-lc htop -d 5"),
        "the daemon must run the startup command as `-lc <command>`; got:\n{got}"
    );

    let (status, body) = http_request(port, "GET", "/api/sessions").await;
    assert_eq!(status, 200, "GET /api/sessions body:\n{body}");
    let list: serde_json::Value = serde_json::from_str(&body).expect("sessions JSON");
    let arr = list.as_array().expect("sessions is an array");
    assert_eq!(arr.len(), 1, "one live console session: {body}");
    assert_eq!(
        arr[0]["kind"], "console",
        "a startup-command console is still a console"
    );
    assert_eq!(
        arr[0]["agent"], "htop -d 5",
        "the session's agent label is the startup command"
    );
}
