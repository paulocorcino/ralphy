//! Single-writer attach policy over `console` channels of real loopback page
//! sockets (docs/adr/0032 §2; issue #166 AC4): two browsers can't corrupt one
//! session. While one client is attached, a second reattach WITHOUT `takeover`
//! is refused: its channel gets no frame and closes as `refused`, and the
//! incumbent keeps the slot. The same reattach WITH `takeover: 1` succeeds,
//! EVICTS the incumbent (its channel ends), and the taker drives the child.
//! Proves the explicit, race-free takeover the policy promises.

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::time::Duration;

use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

#[path = "../support/page.rs"]
pub mod page;

use page::Channel as Ws;

mod attach;
mod closing;
mod watcher;

fn terminal(data: &[u8]) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Terminal {
            session: 1,
            data: data.to_vec(),
        })
        .into(),
    )
}

fn resize(rows: u16, cols: u16) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id: 0,
            verb: "resize".to_string(),
            payload: serde_json::json!({ "rows": rows, "cols": cols }),
        }))
        .into(),
    )
}

/// Drain `ws` until its stream ends, capturing the FIRST `session-end`
/// announcement seen on the way. Returning the reason at all is the assertion:
/// the loop breaks on Close/Err, so a reason here was necessarily read BEFORE the
/// stream ended — the relation issue #334 is about, not a frame count.
async fn drain_capturing_announcement(ws: &mut Ws, secs: u64) -> (bool, Option<String>) {
    let mut announced: Option<String> = None;
    let ended = tokio::time::timeout(Duration::from_secs(secs), async {
        while let Some(msg) = ws.next().await {
            match msg {
                Ok(Message::Binary(bytes)) => {
                    if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                        if cmd.verb == "session-end" && announced.is_none() {
                            announced = cmd
                                .payload
                                .get("reason")
                                .and_then(|v| v.as_str())
                                .map(str::to_string);
                        }
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            }
        }
    })
    .await;
    (ended.is_ok(), announced)
}

/// Read until `needle` (or 10s); answer the `ConPTY` startup `ESC[6n` only when
/// `answer_cursor` (the taker reattaches past startup — see `session_persistence`).
async fn read_until(ws: &mut Ws, needle: &str, answer_cursor: bool) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut acc = String::new();
        while let Some(msg) = ws.next().await {
            let bytes = match msg.expect("read the next frame") {
                Message::Binary(b) => b,
                Message::Text(_)
                | Message::Ping(_)
                | Message::Pong(_)
                | Message::Close(_)
                | Message::Frame(_) => continue,
            };
            if let Ok(Frame::Terminal { data, .. }) = protocol::decode(&bytes) {
                if answer_cursor
                    && data
                        .windows(CURSOR_POSITION_REQUEST.len())
                        .any(|w| w == CURSOR_POSITION_REQUEST)
                {
                    ws.send(terminal(CURSOR_POSITION_REPLY))
                        .await
                        .expect("send the frame");
                }
                acc.push_str(&String::from_utf8_lossy(&data));
                if acc.contains(needle) {
                    return acc;
                }
            }
        }
        acc
    })
    .await
    .unwrap_or_else(|_| panic!("timed out (10s) waiting for {needle:?}"))
}

/// Boot a router on an ephemeral loopback port with `dir` registered as
/// `owner/workbench`, and point the launcher at the helper child. Returns the
/// port ALONGSIDE the shutdown sender, which the caller must hold: dropping it
/// makes every bridge's `shutdown.changed()` resolve at once, ending each session
/// as a daemon shutdown.
///
/// The env override is set exactly ONCE: the two tests in this binary run in
/// parallel, and a repeated `set_var` across threads is a data race even when
/// both writes carry the same value.
async fn start_daemon(dir: &std::path::Path) -> (u16, tokio::sync::watch::Sender<bool>) {
    static OVERRIDE: std::sync::Once = std::sync::Once::new();
    OVERRIDE.call_once(|| {
        std::env::set_var(
            "RALPHY_DAEMON_AGENT_OVERRIDE",
            env!("CARGO_BIN_EXE_session_test_child"),
        );
    });

    let registry_path = dir.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/workbench", &dir.to_string_lossy());
    registry::save_to(&store, &registry_path).expect("save the registry");

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
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        std::time::Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve the test app");
    });
    (port, tx)
}

/// Raw HTTP over a fresh `TcpStream` (no HTTP client in dev-deps). `(status, body)`.
async fn http_request(port: u16, method: &str, path: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the test server");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .await
        .expect("send the request");
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .await
        .expect("read the response");
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

/// Expect the attach `fields` to be refused: its channel gets no frame (no
/// `session-open`), then `channel.closed` with reason `refused`.
async fn refused(port: u16, fields: serde_json::Value, why: &str) {
    let mut channel = page::console(port, fields).await;
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
    .unwrap_or_else(|_| panic!("{why}: the daemon ends the attach within 10s"));
    assert!(
        frames.is_empty(),
        "{why}: a refused attach sends no frame: {frames:?}"
    );
    assert_eq!(channel.closed_reason(), Some("refused"), "{why}");
}
