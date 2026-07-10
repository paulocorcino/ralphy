//! Session persistence over a real loopback WebSocket (docs/adr/0032 §2; issue
//! #166): the tmux model. A client launches a session, types a marker, then drops
//! its socket — and the session survives. `GET /api/sessions` still lists it (the
//! child is alive); a reattach by id first REPLAYS the scrollback (the earlier
//! marker) and then STREAMS LIVE (a freshly typed marker arrives); and
//! `POST /api/sessions/close` removes it. Proves AC1 (detach/reattach + replay)
//! and AC3 (list with identity + close), the inverse of #162's WS-drop teardown.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

type Ws = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Encode a terminal keystroke frame the way the browser would.
fn terminal(data: &[u8]) -> Message {
    Message::Binary(protocol::encode(&Frame::Terminal {
        session: 1,
        data: data.to_vec(),
    }))
}

/// Read decoded terminal output until `needle` appears (or 10s). When
/// `answer_cursor`, plays the terminal emulator by answering the ConPTY startup
/// `ESC[6n` so the child unblocks on Windows. On a REATTACH the child is already
/// past startup and the replayed scrollback carries that startup `ESC[6n` as
/// history — re-answering it would inject a stray `ESC[1;1R` into the child's
/// stdin that prepends to the next typed line — so callers pass `false` there.
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

/// A raw HTTP/1.1 request over a fresh `TcpStream` against the live `axum::serve`
/// listener. The crate's dev-deps carry no general HTTP client and the session
/// endpoints are plain GET/POST, so this avoids a new dependency. Returns
/// `(status_code, body)`.
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

/// Reattach by id, retrying briefly on a `409`: a just-dropped socket's server
/// bridge clears the single-writer slot a hair after the client observes the
/// close, so an immediate reattach can race it. A browser would retry the same way.
async fn reattach(port: u16, id: u64) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/ws/session?id={id}");
    for _ in 0..40 {
        match tokio_tungstenite::connect_async(&url).await {
            Ok((ws, _)) => return ws,
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    panic!("reattach to session {id} never succeeded within 2s");
}

#[tokio::test]
async fn session_survives_ws_drop_reattach_replays_then_streams_and_close_removes() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/workbench";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    // Point the launcher at the helper bin. This file's sole test → no env race.
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
        std::time::Instant::now(),
        rx,
        ralphy_daemon::auth::AuthPolicy::Localhost,
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // ws1: launch, type a marker, read it echoed, then DROP the socket.
    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fworkbench&agent=claude");
    let (mut ws1, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting to /ws/session");
    ws1.send(terminal(b"marker-alpha\r")).await.unwrap();
    read_until(&mut ws1, "GOT:marker-alpha", true).await;
    ws1.close(None).await.unwrap();
    drop(ws1);

    // The session must OUTLIVE the socket: the child is alive, so the list still
    // carries it, with its identity (repo slug + agent).
    let (status, body) = http_request(port, "GET", "/api/sessions").await;
    assert_eq!(status, 200, "GET /api/sessions body:\n{body}");
    let list: serde_json::Value = serde_json::from_str(&body).expect("sessions JSON");
    let arr = list.as_array().expect("sessions is an array");
    assert_eq!(arr.len(), 1, "the dropped session must still be listed: {body}");
    let entry = &arr[0];
    assert_eq!(entry["repo"], slug, "listed session carries its repo slug");
    assert_eq!(entry["agent"], "claude", "listed session carries its agent");
    assert_eq!(entry["kind"], "agent");
    assert!(entry["started_at"].as_u64().is_some(), "started_at present");
    let id = entry["id"].as_u64().expect("session id");

    // ws2: reattach by id. The FIRST thing we must see is the REPLAYED scrollback
    // containing the marker typed on ws1 (proves the ring survived the detach).
    let mut ws2 = reattach(port, id).await;
    read_until(&mut ws2, "GOT:marker-alpha", false).await;

    // Then LIVE streaming resumes: a freshly typed marker echoes back, which is
    // only possible if the child stayed alive across the detach/reattach.
    ws2.send(terminal(b"marker-beta\r")).await.unwrap();
    read_until(&mut ws2, "GOT:marker-beta", false).await;

    // Close removes it: 200, then the list no longer contains the id.
    let (status, body) = http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
    assert_eq!(status, 200, "close body:\n{body}");
    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    assert!(
        !body.contains(&format!("\"id\":{id}")),
        "closed session must be gone from the list; got:\n{body}"
    );
}
