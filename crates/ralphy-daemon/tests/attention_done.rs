//! The done mark over HTTP (docs/adr/0085 D11): a Claude console whose window
//! record has "tell me when it is done" opens one `console-done` request at
//! its next finished turn, never at a session start, and only once. The
//! request can be dismissed.
//!
//! "The hook" is this test, appending lines to the console's status file
//! under `<store>/sessions/`; the agent is `session_test_child`. The store and
//! the agent override are process-global environment, so each test needs a
//! process of its own (nextest runs each test in its own process).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ralphy_daemon::auth::AuthState;
use ralphy_daemon::desk::{self, DeskRecord, DeskRect, DeskStore};
use ralphy_daemon::{registry, router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const SLUG: &str = "owner/attention";
const RECORD: &str = "w-1";

const WORKING: &str = r#"{"event":"UserPromptSubmit","ts":"2026-10-29T10:00:00-03:00"}"#;
const SESSION_START: &str = r#"{"event":"SessionStart","ts":"2026-10-29T10:00:05-03:00"}"#;
const STOP: &str = r#"{"event":"Stop","ts":"2026-10-29T10:00:09-03:00"}"#;

/// The status and the body of the reply to `request`.
async fn exchange(port: u16, request: &str) -> (u16, String) {
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the daemon");
    sock.write_all(request.as_bytes())
        .await
        .expect("send the request");
    let mut raw = String::new();
    sock.read_to_string(&mut raw).await.expect("read the reply");
    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_default();
    let body = raw.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    (status, body.to_string())
}

async fn get(port: u16, path: &str) -> (u16, serde_json::Value) {
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    let (status, body) = exchange(port, &request).await;
    (status, serde_json::from_str(&body).unwrap_or_default())
}

/// The status of `POST /api/attention/dismiss {"id"}`.
async fn dismiss(port: u16, id: &str) -> u16 {
    let body = serde_json::json!({ "id": id }).to_string();
    let request = format!(
        "POST /api/attention/dismiss HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    exchange(port, &request).await.0
}

/// The requests of the list once `ready` accepts them; panics after 10 s.
async fn requests_when(
    port: u16,
    ready: impl Fn(&[serde_json::Value]) -> bool,
) -> Vec<serde_json::Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (status, reply) = get(port, "/api/attention").await;
        let list = reply["requests"].as_array().cloned().unwrap_or_default();
        if status == 200 && ready(&list) {
            return list;
        }
        assert!(
            Instant::now() < deadline,
            "no such list within 10 s: {reply}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The requests of the list now.
async fn requests_now(port: u16) -> Vec<serde_json::Value> {
    let (_, reply) = get(port, "/api/attention").await;
    reply["requests"].as_array().cloned().unwrap_or_default()
}

/// A full tick of the attention list, and a little more.
async fn after_a_tick() {
    tokio::time::sleep(Duration::from_millis(3600)).await;
}

fn append(status: &Path, line: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(status)
        .expect("the console's status file exists");
    writeln!(f, "{line}").expect("append a hook line");
}

/// A daemon over `dir/home` (registered as `SLUG`) whose desk holds the
/// window `RECORD` with the done mark on, and a Claude console launched for
/// that record. Returns the port, the console's status file, the desk file and
/// the socket that keeps the console attached.
async fn marked_console(dir: &Path) -> (u16, PathBuf, PathBuf, Ws) {
    let store = dir.join("store");
    std::fs::create_dir_all(&store).expect("create the store");
    std::env::set_var("RALPHY_DAEMON_DIR", &store);
    std::env::set_var(
        "RALPHY_DAEMON_AGENT_OVERRIDE",
        env!("CARGO_BIN_EXE_session_test_child"),
    );
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("create the repo dir");
    let registry_path = home.join("repos.toml");
    let mut reg = registry::RegistryStore::default();
    reg.upsert(SLUG, &home.to_string_lossy());
    registry::save_to(&reg, &registry_path).expect("save the registry");
    let desk_path = home.join("desk.toml");
    let marked = DeskStore {
        windows: vec![DeskRecord {
            id: RECORD.to_string(),
            repo: SLUG.to_string(),
            agent: "claude".to_string(),
            kind: "agent".to_string(),
            rect: DeskRect {
                left: 10.0,
                top: 10.0,
                width: 640.0,
                height: 480.0,
            },
            done_mark: true,
            ..DeskRecord::default()
        }],
        ..DeskStore::default()
    };
    desk::save_to(&marked, &desk_path).expect("write the desk");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a port");
    let port = listener.local_addr().expect("a bound address").port();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        AuthState::localhost(),
    );
    tokio::spawn(async move {
        // The shutdown sender lives as long as the server.
        let _keep = tx;
        axum::serve(listener, app).await.expect("the daemon serves");
    });

    let url = format!(
        "ws://127.0.0.1:{port}/ws/session?repo=owner%2Fattention&agent=claude&record={RECORD}"
    );
    let request = url.into_client_request().expect("a websocket request");
    let (console, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("a claude console launches");
    let status = store.join("sessions").join("1.agent-status.jsonl");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !status.is_file() {
        assert!(Instant::now() < deadline, "the console never started");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (port, status, desk_path, console)
}

fn ids(requests: &[serde_json::Value]) -> Vec<&str> {
    requests.iter().filter_map(|r| r["id"].as_str()).collect()
}

fn mark_is_on(desk_path: &Path) -> bool {
    desk::load_from(desk_path)
        .expect("the desk reads")
        .windows
        .iter()
        .any(|w| w.id == RECORD && w.done_mark)
}

#[tokio::test]
async fn the_done_mark_fires_on_a_turn_end_and_not_on_a_session_boundary() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (port, status, desk_path, _console) = marked_console(dir.path()).await;

    // A session that starts after work is a `done`, but no turn ended.
    append(&status, WORKING);
    append(&status, SESSION_START);
    after_a_tick().await;
    assert!(requests_now(port).await.is_empty());
    assert!(mark_is_on(&desk_path), "a session start leaves the mark on");

    append(&status, WORKING);
    append(&status, STOP);
    let requests = requests_when(port, |r| !r.is_empty()).await;
    assert_eq!(ids(&requests), ["done:1:1"], "{requests:?}");
    let request = &requests[0];
    assert_eq!(request["kind"], "console-done", "{request}");
    assert_eq!(request["vendor"], "claude", "{request}");
    assert_eq!(request["console"], "1", "{request}");
    assert_eq!(request["repo"], SLUG, "{request}");
    assert_eq!(request["since"], "2026-10-29T10:00:09-03:00", "{request}");
    assert_eq!(request["dismissable"], true, "{request}");
}

#[tokio::test]
async fn the_done_mark_fires_once() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (port, status, desk_path, _console) = marked_console(dir.path()).await;

    append(&status, WORKING);
    append(&status, STOP);
    requests_when(port, |r| ids(r) == ["done:1:1"]).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while mark_is_on(&desk_path) {
        assert!(
            Instant::now() < deadline,
            "the mark stayed on after it fired"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    append(&status, WORKING);
    append(&status, STOP);
    after_a_tick().await;
    let requests = requests_now(port).await;
    assert_eq!(
        ids(&requests),
        ["done:1:1"],
        "a second finished turn opens no request: {requests:?}"
    );
}

#[tokio::test]
async fn a_done_request_can_be_dismissed() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (port, status, _desk, _console) = marked_console(dir.path()).await;

    append(&status, WORKING);
    append(&status, STOP);
    requests_when(port, |r| ids(r) == ["done:1:1"]).await;
    assert_eq!(dismiss(port, "done:1:1").await, 200);
    requests_when(port, <[serde_json::Value]>::is_empty).await;
}
