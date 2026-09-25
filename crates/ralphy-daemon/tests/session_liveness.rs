//! The session bridge's liveness rule (ADR-0051 §9 amendment 2026-09-22): a
//! writer that answers no ping for two ping periods is gone, and its slot is
//! released for the next attach. A send succeeding proves nothing — a tunnel
//! agent keeps its leg to the daemon open after the browser behind it vanished —
//! so only what the CLIENT sends counts. A live client, which pongs, is never
//! released.
//!
//! Its own binary: the ping period is shortened through
//! `RALPHY_DAEMON_WS_PING_MS`, read once per process, and the other session
//! tests must keep the production period.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::WebSocketStream;

type Ws = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 500 ms pings, so a silent client is released after 1125 ms. The daemon and
/// the client share the test's one thread, so a stall of the whole thread reads
/// as silence: at 200 ms (450 ms window) a loaded macOS runner released a
/// client that was answering (CI run 36109548565, 2026-09-25).
const PING_MS: u64 = 500;
/// Past the release: the silence window plus a whole ping tick, doubled for a
/// loaded CI host.
const PAST_RELEASE: Duration = Duration::from_millis(2 * (PING_MS * 9 / 4 + PING_MS));

fn terminal(data: &[u8]) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Terminal {
            session: 1,
            data: data.to_vec(),
        })
        .into(),
    )
}

/// Read until `needle` (or 10s), answering the ConPTY startup `ESC[6n`.
async fn read_until(ws: &mut Ws, needle: &str) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut acc = String::new();
        while let Some(msg) = ws.next().await {
            let bytes = match msg.expect("reading the session stream") {
                Message::Binary(b) => b,
                _ => continue,
            };
            if let Ok(Frame::Terminal { data, .. }) = protocol::decode(&bytes) {
                if data
                    .windows(CURSOR_POSITION_REQUEST.len())
                    .any(|w| w == CURSOR_POSITION_REQUEST)
                {
                    ws.send(terminal(CURSOR_POSITION_REPLY))
                        .await
                        .expect("answering the cursor request");
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

async fn start_daemon(dir: &std::path::Path) -> (u16, tokio::sync::watch::Sender<bool>) {
    static OVERRIDE: std::sync::Once = std::sync::Once::new();
    OVERRIDE.call_once(|| {
        std::env::set_var(
            "RALPHY_DAEMON_AGENT_OVERRIDE",
            env!("CARGO_BIN_EXE_session_test_child"),
        );
        std::env::set_var("RALPHY_DAEMON_WS_PING_MS", PING_MS.to_string());
    });

    let registry_path = dir.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/workbench", &dir.to_string_lossy());
    registry::save_to(&store, &registry_path).expect("writing the registry");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding a loopback port");
    let port = listener.local_addr().expect("the bound address").port();
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
            .expect("serving the router");
    });
    (port, tx)
}

async fn http_request(port: u16, method: &str, path: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connecting for HTTP");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .await
        .expect("sending the request");
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .await
        .expect("reading the response");
    let text = String::from_utf8_lossy(&buf).into_owned();
    text.split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default()
}

/// Launch a session and round-trip a keystroke, so the returned socket is its
/// attached writer. Returns the socket and the session id.
async fn launch(port: u16) -> (Ws, u64) {
    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fworkbench&agent=claude");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("launching a session");
    ws.send(terminal(b"first\r"))
        .await
        .expect("typing into the session");
    read_until(&mut ws, "GOT:first").await;
    let body = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).expect("the session list is JSON");
    let id = list[0]["id"].as_u64().expect("the launched session's id");
    (ws, id)
}

async fn attach_status(port: u16, id: u64) -> Result<Ws, u16> {
    let url = format!("ws://127.0.0.1:{port}/ws/session?id={id}");
    match tokio_tungstenite::connect_async(&url).await {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
        Err(other) => panic!("the reattach failed below HTTP: {other:?}"),
    }
}

/// The half-open leg: the writer's socket stays open but nothing reads it, so
/// no pong ever goes back — what the daemon sees when a tunnel agent holds the
/// leg for a browser that is gone.
#[tokio::test]
async fn a_writer_that_answers_no_ping_is_released() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (port, _shutdown) = start_daemon(dir.path()).await;
    let (silent, id) = launch(port).await;

    assert_eq!(
        attach_status(port, id).await.err(),
        Some(409),
        "a writer heard from moments ago still holds the slot"
    );

    tokio::time::sleep(PAST_RELEASE).await;
    let mut next = attach_status(port, id)
        .await
        .expect("a writer silent past two ping periods must have released the slot");
    next.send(terminal(b"after\r"))
        .await
        .expect("typing as the new writer");
    read_until(&mut next, "GOT:after").await;

    drop(silent);
    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
}

/// The other half, so the rule cannot pass by releasing everyone: a writer that
/// keeps reading pongs every ping and holds the slot through many windows.
#[tokio::test]
async fn a_writer_that_pongs_keeps_the_slot() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (port, _shutdown) = start_daemon(dir.path()).await;
    let (mut live, id) = launch(port).await;

    // Reading is what sends the pongs (tungstenite answers a ping on the next read).
    let reader = tokio::spawn(async move {
        while let Some(Ok(_)) = live.next().await {}
        live
    });
    tokio::time::sleep(PAST_RELEASE * 2).await;
    assert_eq!(
        attach_status(port, id).await.err(),
        Some(409),
        "a writer that answers its pings must keep the slot"
    );

    reader.abort();
    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
}
