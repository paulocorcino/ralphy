//! The pushes of the shown facts the daemon owns (ADR-0070 D2 event 1), read
//! off a real `/ws` socket: `desk.dirty` after a PUT that changes the desk,
//! and none after a PUT that changes nothing.

use std::path::Path;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::router;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Serve a router rooted at `dir` (its `desk.toml` and `peers/` are siblings
/// of `repos.toml`) and connect a `/ws` presence socket to it.
async fn serve(dir: &Path) -> (u16, Ws) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        dir.join("repos.toml"),
        dir.join("no-config.toml"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        let _hold = tx;
        axum::serve(listener, app).await.unwrap();
    });
    let (ws, _resp) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .expect("connecting to /ws");
    (port, ws)
}

/// The next command frame with `verb` within `within`, skipping heartbeats
/// and other pushes; `None` when none arrives.
async fn next_push(ws: &mut Ws, verb: &str, within: Duration) -> Option<Command> {
    tokio::time::timeout(within, async {
        while let Some(msg) = ws.next().await {
            let Ok(Message::Binary(bytes)) = msg else {
                continue;
            };
            if let Ok(Frame::Command(c)) = protocol::decode(&bytes) {
                if c.verb == verb {
                    return Some(c);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

async fn put_desk(port: u16, tab: &str, body: &str) -> String {
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    sock.write_all(
        format!(
            "PUT /api/desk?tab={tab} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut raw = String::new();
    sock.read_to_string(&mut raw).await.unwrap();
    raw
}

const ONE_WINDOW: &str = r#"{"windows":[{"id":"w-a","repo":"owner/repo","agent":"claude","kind":"console","rect":{"left":10.0,"top":20.0,"width":640.0,"height":480.0},"max":false,"sessionId":7,"ts":1}],"fences":[]}"#;

#[tokio::test]
async fn a_desk_write_that_changes_the_desk_pushes_desk_dirty_with_its_tab() {
    let dir = tempfile::tempdir().unwrap();
    let (port, mut ws) = serve(dir.path()).await;

    let reply = put_desk(port, "t1", ONE_WINDOW).await;
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    let push = next_push(&mut ws, "desk.dirty", Duration::from_secs(5))
        .await
        .expect("desk.dirty after a write that changed the desk");
    assert_eq!(push.id, 0);
    assert_eq!(push.payload["tab"], "t1");

    // Negative control: the same write again changes nothing.
    let reply = put_desk(port, "t1", ONE_WINDOW).await;
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(
        next_push(&mut ws, "desk.dirty", Duration::from_secs(3))
            .await
            .is_none(),
        "a write that changes nothing pushes nothing"
    );
}
