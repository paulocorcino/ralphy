//! `POST /api/command` answers a one-reply command with the same JSON the
//! command socket sends, under the same guard, cap and audit record, and
//! refuses a command whose output streams (ADR-0082 D1, ADR-0072 D3, D11).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::auth::AuthState;
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

const SLUG: &str = "owner/http";
const TOKEN: &str = "http-command-token";

fn app(registry_path: PathBuf) -> axum::Router {
    app_with(registry_path, AuthState::localhost())
}

fn app_with(registry_path: PathBuf, auth: std::sync::Arc<AuthState>) -> axum::Router {
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    router(
        None,
        registry_path,
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        auth,
    )
}

/// A registry in `dir` with one slug whose checkout is `dir/repo`.
fn registered(dir: &Path) -> PathBuf {
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("note.txt"), "hello").unwrap();
    let registry_path = dir.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert(SLUG, &repo.to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    registry_path
}

/// Serve the router on a loopback port, as `serve.rs` does.
async fn serve(registry_path: PathBuf) -> u16 {
    serve_app(app(registry_path)).await
}

async fn serve_app(app: axum::Router) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    port
}

fn command_body(verb: &str, payload: serde_json::Value) -> String {
    serde_json::json!({ "id": 1, "verb": verb, "payload": payload }).to_string()
}

/// `POST /api/command` over a real connection: the status and the JSON body.
async fn over_http(
    port: u16,
    verb: &str,
    payload: serde_json::Value,
    headers: &[(&str, &str)],
) -> (u16, serde_json::Value) {
    let body = command_body(verb, payload);
    let mut extra = String::new();
    for (name, value) in headers {
        extra.push_str(&format!("{name}: {value}\r\n"));
    }
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        extra.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let request = format!(
        "POST /api/command HTTP/1.1\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut reply = String::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_string(&mut reply))
        .await
        .expect("an answer within 20s")
        .unwrap();
    let status: u16 = reply
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("a status line in {reply}"));
    let json = reply
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("");
    let value = serde_json::from_str(json).unwrap_or(serde_json::Value::Null);
    (status, value)
}

/// The one reply payload `/ws/command` sends for the same command.
async fn over_socket(
    port: u16,
    verb: &str,
    payload: serde_json::Value,
    headers: &[(&str, &str)],
) -> serde_json::Value {
    let mut request = format!("ws://127.0.0.1:{port}/ws/command")
        .into_client_request()
        .unwrap();
    for (name, value) in headers {
        request.headers_mut().insert(
            header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let frame = Frame::Command(Command {
        id: 1,
        verb: verb.to_string(),
        payload,
    });
    ws.send(Message::Binary(protocol::encode(&frame).into()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(msg) = ws.next().await {
            let Ok(Message::Binary(bytes)) = msg else {
                continue;
            };
            if let Ok(Frame::Command(reply)) = protocol::decode(&bytes) {
                return reply.payload;
            }
        }
        panic!("the socket closed with no reply")
    })
    .await
    .expect("a reply within 20s")
}

#[tokio::test]
async fn a_read_answers_the_same_over_http_and_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let port = serve(registered(dir.path())).await;
    let payload = serde_json::json!({ "repo": SLUG, "path": "" });
    let (status, http) = over_http(port, "tree.list", payload.clone(), &[]).await;
    let socket = over_socket(port, "tree.list", payload, &[]).await;
    assert_eq!(status, 200);
    assert!(http["entries"].is_array(), "{http}");
    assert_eq!(http, socket);
}

#[tokio::test]
async fn a_write_answers_the_same_over_http_and_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let port = serve(registered(dir.path())).await;
    let payload = serde_json::json!({ "repo": SLUG, "path": "note.txt", "content": "over-http" });
    let socket = over_socket(port, "file.write", payload.clone(), &[]).await;
    std::fs::write(dir.path().join("repo/note.txt"), "hello").unwrap();
    let (status, http) = over_http(port, "file.write", payload, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(http["status"], "ok", "{http}");
    assert_eq!(http, socket);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("repo/note.txt")).unwrap(),
        "over-http"
    );
}

// 3 MB is over axum's 2 MB default body limit and under the command cap,
// so only the route's own limit lets this body through.
#[tokio::test]
async fn a_write_of_three_megabytes_is_answered_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let port = serve(registered(dir.path())).await;
    let content = "y".repeat(3_000_000);
    let payload = serde_json::json!({ "repo": SLUG, "path": "big.txt", "content": content });
    let (status, http) = over_http(port, "file.write", payload, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(http["status"], "ok", "{http}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("repo/big.txt")).unwrap(),
        content
    );
}

#[tokio::test]
async fn a_refused_verb_answers_the_same_over_http_and_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let port = serve(registered(dir.path())).await;
    let payload = serde_json::json!({ "repo": SLUG });
    let (status, http) = over_http(port, "no.such.verb", payload.clone(), &[]).await;
    let socket = over_socket(port, "no.such.verb", payload, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(
        http,
        serde_json::json!({ "status": "error", "message": "unknown verb" })
    );
    assert_eq!(http, socket);
}

// The only test of this file that sets the exe override: no other test here
// spawns, so the process-global variables race with nothing.
#[tokio::test]
async fn a_streaming_verb_is_refused_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran.marker");
    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_DONE_FILE", &marker);
    let port = serve(registered(dir.path())).await;
    let payload = serde_json::json!({ "repo": SLUG, "agent": "claude", "branchMode": "new" });
    let (status, http) = over_http(port, "run", payload, &[]).await;
    assert_eq!(status, 200);
    assert_eq!(http["status"], "error", "{http}");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!marker.exists(), "a streaming verb spawned a process");
}

#[tokio::test]
async fn a_command_over_the_cap_is_answered_413() {
    let dir = tempfile::tempdir().unwrap();
    let body = "x".repeat(ralphy_daemon::tree::MAX_COMMAND_BYTES + 1);
    let status = app(registered(dir.path()))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/command")
                .header(header::HOST, "127.0.0.1:7257")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("the router answers")
        .status();
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn a_command_from_another_origin_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let body = command_body("tree.list", serde_json::json!({ "repo": SLUG, "path": "" }));
    let status = app(registered(dir.path()))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/command")
                .header(header::HOST, "127.0.0.1:7257")
                .header(header::ORIGIN, "https://evil.example")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("the router answers")
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_write_over_http_writes_one_command_audit_line() {
    let dir = tempfile::tempdir().unwrap();
    let port = serve(registered(dir.path())).await;
    let payload = serde_json::json!({ "repo": SLUG, "path": "note.txt", "content": "audited" });
    let (status, _) = over_http(port, "file.write", payload, &[]).await;
    assert_eq!(status, 200);
    let log = std::fs::read_to_string(dir.path().join("daemon-audit.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .filter(|line: &serde_json::Value| line["event"] != "device-seen")
        .collect();
    assert_eq!(lines.len(), 1, "{log}");
    assert_eq!(lines[0]["event"], "command", "{log}");
    assert_eq!(lines[0]["verb"], "file.write", "{log}");
}

#[tokio::test]
async fn a_host_password_over_plain_http_is_refused_over_http_too() {
    let dir = tempfile::tempdir().unwrap();
    // A daemon bound to its LAN address, asked by name of that address over
    // plain http: the machine bearer passes the guard, the password must not.
    // The auth state reads its store: point it at an empty one, not the
    // operator's (nextest runs each test in its own process).
    std::env::set_var("RALPHY_DAEMON_DIR", dir.path());
    let bound: std::net::SocketAddr = "192.168.0.10:7257".parse().unwrap();
    let auth = AuthState::boot(
        bound,
        Some(TOKEN.to_string()),
        SessionEpoch::in_memory_detached(),
        &[],
    )
    .expect("a bind with a token boots");
    let port = serve_app(app_with(registered(dir.path()), auth)).await;
    let payload = serde_json::json!({ "destination": "svrapp", "password": "s3 cret" });
    let bearer = format!("Bearer {TOKEN}");
    let lan = [
        ("host", "192.168.0.10:7257"),
        ("authorization", bearer.as_str()),
    ];
    let (status, http) = over_http(port, "host.add", payload.clone(), &lan).await;
    let socket = over_socket(port, "host.add", payload, &lan).await;
    assert_eq!(status, 200);
    assert!(
        http["message"]
            .as_str()
            .unwrap_or("")
            .starts_with("A password is accepted only over https or from this computer"),
        "{http}"
    );
    assert_eq!(http, socket);
}
