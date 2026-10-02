//! The pushes of the shown facts the daemon owns (ADR-0070 D2 event 1), read
//! off a real `/ws` socket: `desk.dirty` after a PUT that changes the desk
//! and after a new desk, none after a PUT that changes nothing, and
//! `peers.dirty` / `repos.dirty` when another process writes those stores.

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

async fn request(port: u16, method: &str, path: &str, body: &str) -> String {
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    sock.write_all(
        format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

async fn put_desk(port: u16, tab: &str, body: &str) -> String {
    request(port, "PUT", &format!("/api/desk?tab={tab}"), body).await
}

/// Change a store with `change(n)` until `verb` arrives. The watch takes its
/// first stamp at a time the test does not know, so a change made before it
/// would go unseen: each round makes a new change and waits one stamp
/// period. `None` when no push arrives within 20 s.
async fn change_until_push(
    ws: &mut Ws,
    verb: &str,
    mut change: impl FnMut(u32),
) -> Option<Command> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut n = 0;
    while Instant::now() < deadline {
        change(n);
        n += 1;
        if let Some(c) = next_push(ws, verb, Duration::from_secs(3)).await {
            return Some(c);
        }
    }
    None
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

/// `ralphy host …` writes the peer store from another process; the daemon
/// sees the file change and pushes `peers.dirty`.
#[tokio::test]
async fn a_new_peer_file_pushes_peers_dirty() {
    let dir = tempfile::tempdir().unwrap();
    let (_port, mut ws) = serve(dir.path()).await;
    let peers = dir.path().join("peers");
    std::fs::create_dir_all(&peers).unwrap();
    let push = change_until_push(&mut ws, "peers.dirty", |n| {
        std::fs::write(peers.join(format!("x{n}.toml")), "x").unwrap();
    })
    .await;
    assert!(push.is_some(), "peers.dirty after the peer store changed");
}

/// `ralphy add` writes the project registry from another process; the daemon
/// sees the file change and pushes `repos.dirty`.
#[tokio::test]
async fn a_changed_project_registry_pushes_repos_dirty() {
    let dir = tempfile::tempdir().unwrap();
    let (_port, mut ws) = serve(dir.path()).await;
    let registry = dir.path().join("repos.toml");
    let push = change_until_push(&mut ws, "repos.dirty", |n| {
        // A new length each round: the stamp sees it even on a file system
        // that stores times to the second.
        std::fs::write(&registry, "#".repeat(n as usize + 1)).unwrap();
    })
    .await;
    assert!(push.is_some(), "repos.dirty after the registry changed");
}

/// Starting a new desk changes the desk every tab shows, so it pushes
/// `desk.dirty` like a write does.
#[tokio::test]
async fn starting_a_new_desk_pushes_desk_dirty() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("desk.toml"), "not a toml { ][").unwrap();
    let (port, mut ws) = serve(dir.path()).await;
    let reply = request(port, "POST", "/api/desk/new", "").await;
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(
        next_push(&mut ws, "desk.dirty", Duration::from_secs(5))
            .await
            .is_some(),
        "desk.dirty after a new desk"
    );
}

/// ADR-0070 D6: the page and the presence frame name the same build.
#[tokio::test]
async fn the_page_and_the_presence_frame_carry_one_build_id() {
    let dir = tempfile::tempdir().unwrap();
    let (port, mut ws) = serve(dir.path()).await;
    let id = ralphy_daemon::assets::build_id();

    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    sock.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut page = String::new();
    sock.read_to_string(&mut page).await.unwrap();
    let tag = format!(r#"<meta name="ralphy-build" content="{id}">"#);
    assert!(page.contains(&tag), "GET / carries {tag}");

    let presence = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(msg) = ws.next().await {
            let Ok(Message::Binary(bytes)) = msg else {
                continue;
            };
            if let Ok(Frame::Presence(p)) = protocol::decode(&bytes) {
                return Some(p);
            }
        }
        None
    })
    .await
    .expect("a heartbeat within 5 s")
    .expect("a presence frame");
    assert_eq!(presence.build.as_deref(), Some(id));
}
