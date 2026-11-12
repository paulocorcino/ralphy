//! `GET /api/attention` over HTTP (docs/adr/0085 D10, D11): a Claude console
//! whose hooks report `waiting` is one request, and it leaves the list when
//! the state moves on. The daemon the page talks to joins each peer's
//! `?local=1` list, and names a peer that did not answer. A run whose agent
//! waits is listed from the snapshot it writes in its registered repo.
//!
//! "The hook" is this test, appending lines to the console's status file
//! under `<store>/sessions/`; the agent is `session_test_child`.
//!
//! The store and the agent override are process-global environment, so each
//! test needs a process of its own (nextest runs each test in its own
//! process).
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]
#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

#[path = "../support/golden.rs"]
mod golden;
#[path = "../support/page.rs"]
pub mod page;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ralphy_daemon::auth::{AuthPolicy, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity::Identity;
use ralphy_daemon::peer::{self, PeerDescriptor, PEER_PROTOCOL_VERSION};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use page::Ws;

mod dismissal;
mod peer_dismissal;
mod peer_list;
mod run_snapshot;
mod waiting_console;

const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const PEER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const DEAD_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const SLUG: &str = "owner/attention";
const TOKEN: &str = "peer-token";

/// The status, the header block and the body of one request.
async fn http_get(port: u16, path: &str) -> (u16, String, String) {
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    exchange(port, &request).await
}

/// The status, the header block and the body of the reply to `request`.
async fn exchange(port: u16, request: &str) -> (u16, String, String) {
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
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    (status, head.to_owned(), body.to_owned())
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// The reply of `path` once `ready` accepts its `requests`; panics after 10 s.
async fn wait_for(
    port: u16,
    path: &str,
    ready: impl Fn(&serde_json::Value) -> bool,
) -> (String, serde_json::Value) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (status, head, body) = http_get(port, path).await;
        let reply: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        if status == 200 && ready(&reply["requests"]) {
            return (head, reply);
        }
        assert!(
            Instant::now() < deadline,
            "no such list within 10 s: {status} {body}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The payload of the next `verb` push on the presence socket; panics after 10 s.
async fn next_push(ws: &mut Ws, verb: &str) -> serde_json::Value {
    use futures_util::StreamExt;
    let read = async {
        while let Some(msg) = ws.next().await {
            let Ok(tokio_tungstenite::tungstenite::Message::Binary(bytes)) = msg else {
                continue;
            };
            if let Ok(Frame::Command(c)) = protocol::decode(&bytes) {
                if c.verb == verb {
                    return Some(c.payload);
                }
            }
        }
        None
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .map_err(|elapsed| format!("no {verb} within 10 s ({elapsed})"))
        .expect("this test needs the call to succeed")
        .ok_or_else(|| format!("the presence socket closed before {verb}"))
        .expect("this test needs the value")
}

fn append(status: &Path, line: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(status)
        .expect("the console's status file exists");
    writeln!(f, "{line}").expect("append a hook line");
}

const PERMISSION: &str = concat!(
    r#"{"event":"PermissionRequest","tool_name":"Bash","tool_input":null,"#,
    r#""ts":"2026-10-28T10:00:00-03:00"}"#
);
const ANSWERED: &str = concat!(
    r#"{"event":"PostToolUse","tool_name":"Bash","tool_input":null,"#,
    r#""ts":"2026-10-28T10:00:05-03:00"}"#
);

/// The process-wide store and agent: one store for every daemon of the test.
fn prepare_environment(dir: &Path) -> PathBuf {
    let store = dir.join("store");
    std::fs::create_dir_all(&store).expect("create the store");
    std::env::set_var("RALPHY_DAEMON_DIR", &store);
    std::env::set_var(
        "RALPHY_DAEMON_AGENT_OVERRIDE",
        env!("CARGO_BIN_EXE_session_test_child"),
    );
    store
}

/// A daemon in-process over the registry in `home`, which registers `SLUG`
/// at `home`. Returns its port.
async fn serve(home: &Path, identity: Option<Identity>, auth: Arc<AuthState>) -> u16 {
    let registry_path = home.join("repos.toml");
    let mut reg = registry::RegistryStore::default();
    reg.upsert(SLUG, &home.to_string_lossy());
    registry::save_to(&reg, &registry_path).expect("save the registry");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a port");
    let port = listener.local_addr().expect("a bound address").port();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        identity,
        registry_path,
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        auth,
    );
    tokio::spawn(async move {
        // The shutdown sender lives as long as the server.
        let _keep = tx;
        axum::serve(listener, app).await.expect("the daemon serves");
    });
    port
}

/// A Claude console on the daemon at `port`, with `bearer` when it asks one.
async fn claude_console(port: u16, bearer: Option<&str>) -> page::Channel {
    let mut request = page::url(port)
        .into_client_request()
        .expect("a websocket request");
    if let Some(token) = bearer {
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {token}").parse().expect("a header value"),
        );
    }
    let ws = page::connect_request(request).await;
    let fields = serde_json::json!({ "repo": "owner/attention", "agent": "claude" });
    let mut console = page::Channel::open(ws, "console", fields).await;
    // The daemon starts the session after it takes the open: the first frame
    // is `session-open`, so the status file exists once this returns.
    let first = tokio::time::timeout(Duration::from_secs(10), console.next())
        .await
        .expect("the daemon opens the console within 10 s")
        .expect("the console channel yields a frame")
        .expect("the page socket reads");
    let bytes = if let tokio_tungstenite::tungstenite::Message::Binary(bytes) = first {
        Ok(bytes)
    } else {
        Err(format!(
            "the first message of a console is a frame: {first:?}"
        ))
    };
    let frame = protocol::decode(&bytes.expect("this test needs a frame"));
    assert!(
        matches!(&frame, Ok(Frame::Command(open)) if open.verb == "session-open"),
        "the first frame of a console is session-open: {frame:?}"
    );
    console
}

fn identity(id: &str, name: &str) -> Identity {
    Identity {
        id: id.parse().expect("a daemon id"),
        name: name.to_owned(),
        avatar: "🐙".to_owned(),
    }
}

fn descriptor(id: &str, port: u16) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: id.to_owned(),
        name: "peer".to_owned(),
        avatar: "🐙".to_owned(),
        address: "127.0.0.1".to_owned(),
        port,
        environment: "Linux".to_owned(),
        os: String::new(),
        token: TOKEN.to_owned(),
        protocol_version: PEER_PROTOCOL_VERSION,
        tunnel: None,
        nudge: None,
    }
}

/// The status and the body of `POST /api/attention/dismiss {"id"}`.
async fn dismiss(port: u16, id: &str) -> (u16, String) {
    let body = serde_json::json!({ "id": id }).to_string();
    let request = format!(
        "POST /api/attention/dismiss HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (status, _, body) = exchange(port, &request).await;
    (status, body)
}

const STAMP: &str = "20261028-100000";

/// A finished run in the folder `STAMP` of the repo at `home`: its final
/// snapshot, whose issues 71 and 72 ended `blocked`.
fn finished_run(home: &Path) {
    let run_dir = home.join(".ralphy").join("runs").join(STAMP);
    std::fs::create_dir_all(&run_dir).expect("create the run folder");
    let snapshot = ralphy_run_snapshot::RunSnapshot {
        runid: "01RUNFINISHED".to_owned(),
        pid: 1,
        exec_agent: "codex".to_owned(),
        started_at: "2026-10-28T10:00:00-03:00".to_owned(),
        issues: [71, 72]
            .into_iter()
            .map(|number| ralphy_run_snapshot::IssueBlock {
                number,
                status: "blocked".to_owned(),
                ..ralphy_run_snapshot::IssueBlock::default()
            })
            .collect(),
        ..ralphy_run_snapshot::RunSnapshot::default()
    };
    std::fs::write(
        ralphy_run_snapshot::final_snapshot_path(&run_dir),
        serde_json::to_vec(&snapshot).expect("a snapshot serializes"),
    )
    .expect("write the final snapshot");
}

fn ids(requests: &serde_json::Value) -> Vec<String> {
    requests
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| r["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn issue(n: u64) -> String {
    format!("issue:{STAMP}:{n}")
}
