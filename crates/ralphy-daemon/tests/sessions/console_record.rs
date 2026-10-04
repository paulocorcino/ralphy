//! One live session per window record (ADR-0050 amendment 2026-10-04). Two
//! pages that load one desk at the same time each relaunch the same free
//! consoles; the launch names its record, so the daemon starts ONE session and
//! the second page attaches to it instead of starting another.

use std::time::Duration;

use futures_util::StreamExt;
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

type Ws = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A daemon over a registry holding one project, on a free loopback port.
async fn daemon(dir: &std::path::Path) -> u16 {
    let registry_path = dir.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/record-repo", &dir.to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    super::point_launcher_at_test_child();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::watch::channel(false);
    // The sender lives as long as the test process: a dropped sender reads as
    // a shutdown to every bridge.
    std::mem::forget(tx);
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
    port
}

/// Launch a free console for `record` as `holder`, and return the socket with
/// the payload of its `session-open`.
async fn launch(port: u16, record: &str, holder: &str) -> (Ws, serde_json::Value) {
    let url = format!(
        "ws://127.0.0.1:{port}/ws/session?console=1&repo=owner%2Frecord-repo&record={record}&holder={holder}"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting to /ws/session");
    let open = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(msg) = ws.next().await {
            let Ok(Message::Binary(bytes)) = msg else {
                continue;
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                assert_eq!(cmd.verb, "session-open", "the first command: {cmd:?}");
                return cmd.payload;
            }
        }
        panic!("the socket closed before session-open");
    })
    .await
    .expect("a session-open within 10s");
    (ws, open)
}

async fn sessions(port: u16) -> Vec<serde_json::Value> {
    let (_, body) = http(port, "GET", "/api/sessions").await;
    serde_json::from_str::<serde_json::Value>(&body)
        .expect("sessions JSON")
        .as_array()
        .expect("sessions is an array")
        .clone()
}

/// A raw HTTP/1.1 request: the crate's dev-deps have no HTTP client.
async fn http(port: u16, method: &str, path: &str) -> (u16, String) {
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
async fn two_launches_of_one_record_start_one_session() {
    let dir = tempfile::tempdir().unwrap();
    let port = daemon(dir.path()).await;

    // Two pages, one record, at the same moment.
    let ((_a, open_a), (_b, open_b)) = tokio::join!(
        launch(port, "w-shared", "tab-a"),
        launch(port, "w-shared", "tab-b"),
    );

    let list = sessions(port).await;
    assert_eq!(list.len(), 1, "one record, one session: {list:?}");
    assert_eq!(list[0]["record"], "w-shared", "{list:?}");
    let id = list[0]["id"].as_u64().expect("session id");
    assert_eq!(open_a["session"], id, "{open_a}");
    assert_eq!(open_b["session"], id, "{open_b}");
    // The page that came second watches: the writer slot is the first page's.
    let watchers = [&open_a, &open_b]
        .iter()
        .filter(|open| open["watch"] == true)
        .count();
    assert_eq!(watchers, 1, "one writer, one watcher: {open_a} {open_b}");
}
