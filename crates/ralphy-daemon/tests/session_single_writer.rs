//! Single-writer attach policy over a real loopback WebSocket (docs/adr/0032 §2;
//! issue #166 AC4): two browsers can't corrupt one session. While one client is
//! attached, a second reattach WITHOUT `takeover` is refused with HTTP `409`
//! BEFORE the upgrade; the same reattach WITH `takeover=1` succeeds, EVICTS the
//! incumbent (its stream ends), and the taker drives the child. Proves the
//! explicit, race-free takeover the policy promises.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::WebSocketStream;

type Ws = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn terminal(data: &[u8]) -> Message {
    Message::Binary(protocol::encode(&Frame::Terminal {
        session: 1,
        data: data.to_vec(),
    }))
}

/// Read until `needle` (or 10s); answer the ConPTY startup `ESC[6n` only when
/// `answer_cursor` (the taker reattaches past startup — see `session_persistence`).
async fn read_until(ws: &mut Ws, needle: &str, answer_cursor: bool) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut acc = String::new();
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                _ => continue,
            };
            if let Ok(Frame::Terminal { data, .. }) = protocol::decode(&bytes) {
                if answer_cursor
                    && data
                        .windows(CURSOR_POSITION_REQUEST.len())
                        .any(|w| w == CURSOR_POSITION_REQUEST)
                {
                    ws.send(terminal(CURSOR_POSITION_REPLY)).await.unwrap();
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

/// Raw HTTP over a fresh `TcpStream` (no HTTP client in dev-deps). `(status, body)`.
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
async fn second_attach_needs_takeover_which_evicts_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/workbench", &dir.path().to_string_lossy());
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
        std::time::Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // ws1: launch and round-trip a keystroke so it is the attached single writer.
    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fworkbench&agent=claude");
    let (mut ws1, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting ws1");
    ws1.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut ws1, "GOT:first", true).await;

    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = list.as_array().unwrap()[0]["id"].as_u64().unwrap();

    // A second attach WITHOUT takeover, while ws1 holds the writer slot, must be
    // refused as HTTP 409 at the handshake — never a silently upgraded socket.
    let busy = format!("ws://127.0.0.1:{port}/ws/session?id={id}");
    match tokio_tungstenite::connect_async(&busy).await {
        Err(tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status(),
                409,
                "a busy session refuses a non-takeover attach"
            );
        }
        Ok(_) => panic!("second attach without takeover must NOT succeed"),
        Err(other) => panic!("expected an HTTP 409, got {other:?}"),
    }

    // The same attach WITH takeover=1 succeeds and evicts ws1.
    let take = format!("ws://127.0.0.1:{port}/ws/session?id={id}&takeover=1");
    let (mut ws2, _) = tokio_tungstenite::connect_async(&take)
        .await
        .expect("takeover attach must succeed");

    // ws1 is evicted: its stream ends within 5s (the server bridge broke and
    // dropped its socket) — NOT a hang.
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(msg) = ws1.next().await {
            match msg {
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            }
        }
    })
    .await;
    assert!(
        ended.is_ok(),
        "the evicted first client's stream must end, not hang"
    );

    // The taker drives the child: a keystroke round-trips (no cursor answer — the
    // child is past startup on this reattach).
    ws2.send(terminal(b"takeover-ok\r")).await.unwrap();
    read_until(&mut ws2, "GOT:takeover-ok", false).await;

    // Close so the helper child (60s-capable in sleep mode) does not linger.
    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
}
