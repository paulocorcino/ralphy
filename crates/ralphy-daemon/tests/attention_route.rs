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

#[path = "support/golden.rs"]
mod golden;

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

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const PEER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const DEAD_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const SLUG: &str = "owner/attention";
const TOKEN: &str = "peer-token";

/// The status, the header block and the body of one request.
async fn http_get(port: u16, path: &str) -> (u16, String, String) {
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the daemon");
    sock.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
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
    (status, head.to_string(), body.to_string())
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
                    return c.payload;
                }
            }
        }
        panic!("the presence socket closed before {verb}");
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .unwrap_or_else(|_| panic!("no {verb} within 10 s"))
}

fn append(status: &Path, line: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(status)
        .expect("the console's status file exists");
    writeln!(f, "{line}").expect("append a hook line");
}

const PERMISSION: &str = r#"{"event":"PermissionRequest","tool_name":"Bash","tool_input":null,"ts":"2026-10-28T10:00:00-03:00"}"#;
const ANSWERED: &str = r#"{"event":"PostToolUse","tool_name":"Bash","tool_input":null,"ts":"2026-10-28T10:00:05-03:00"}"#;

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
async fn claude_console(port: u16, bearer: Option<&str>) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fattention&agent=claude");
    let mut request = url.into_client_request().expect("a websocket request");
    if let Some(token) = bearer {
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {token}").parse().expect("a header value"),
        );
    }
    tokio_tungstenite::connect_async(request)
        .await
        .expect("a claude console launches")
        .0
}

fn identity(id: &str, name: &str) -> Identity {
    Identity {
        id: id.parse().expect("a daemon id"),
        name: name.to_string(),
        avatar: "🐙".to_string(),
    }
}

fn descriptor(id: &str, port: u16) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: id.to_string(),
        name: "peer".to_string(),
        avatar: "🐙".to_string(),
        address: "127.0.0.1".to_string(),
        port,
        environment: "Linux".to_string(),
        os: String::new(),
        token: TOKEN.to_string(),
        protocol_version: PEER_PROTOCOL_VERSION,
        tunnel: None,
        nudge: None,
    }
}

#[tokio::test]
async fn a_waiting_console_enters_and_leaves_the_route() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let store = prepare_environment(dir.path());
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("create the repo dir");
    let port = serve(&home, None, AuthState::localhost()).await;
    let _console = claude_console(port, None).await;

    let (_, reply) = wait_for(port, "/api/attention", |r| r.is_array()).await;
    assert_eq!(reply["requests"], serde_json::json!([]), "{reply}");

    let status = store.join("sessions").join("1.agent-status.jsonl");
    append(&status, PERMISSION);
    let (_, reply) = wait_for(port, "/api/attention", |r| {
        r.as_array().is_some_and(|a| !a.is_empty())
    })
    .await;
    let requests = reply["requests"].as_array().expect("an array");
    assert_eq!(requests.len(), 1, "{reply}");
    let request = &requests[0];
    assert_eq!(request["id"], "console:1", "{reply}");
    assert_eq!(request["kind"], "console-waiting", "{reply}");
    assert_eq!(request["vendor"], "claude", "{reply}");
    assert_eq!(request["console"], "1", "{reply}");
    assert_eq!(request["repo"], SLUG, "{reply}");
    assert_eq!(request["detail"], "permission: Bash", "{reply}");
    assert_eq!(request["since"], "2026-10-28T10:00:00-03:00", "{reply}");
    assert_eq!(request["dismissable"], false, "{reply}");
    golden::check("api-attention", reply.clone(), &[]);

    append(&status, ANSWERED);
    let (_, reply) = wait_for(port, "/api/attention", |r| {
        r.as_array().is_some_and(|a| a.is_empty())
    })
    .await;
    assert_eq!(reply["requests"], serde_json::json!([]), "{reply}");
}

#[tokio::test]
async fn the_list_joins_a_peer_and_names_one_that_did_not_answer() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let store = prepare_environment(dir.path());
    let peer_home = dir.path().join("peer");
    let local_home = dir.path().join("local");
    std::fs::create_dir_all(&peer_home).expect("create the peer repo dir");
    std::fs::create_dir_all(&local_home).expect("create the local repo dir");

    let peer_port = serve(
        &peer_home,
        Some(identity(PEER_ID, "peer")),
        AuthState::fixed(
            AuthPolicy::Bearer(TOKEN.to_string()),
            SessionEpoch::in_memory_detached(),
        ),
    )
    .await;
    peer::write_descriptor(&local_home, &descriptor(PEER_ID, peer_port))
        .expect("announce the peer");
    let dead_port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a port");
        listener.local_addr().expect("a bound address").port()
    };
    peer::write_descriptor(&local_home, &descriptor(DEAD_ID, dead_port))
        .expect("announce the dead peer");
    let local_port = serve(
        &local_home,
        Some(identity(LOCAL_ID, "local")),
        AuthState::localhost(),
    )
    .await;

    // The only console runs on the peer, so its status file is the only one.
    let _console = claude_console(peer_port, Some(TOKEN)).await;
    let status = store.join("sessions").join("1.agent-status.jsonl");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !status.is_file() {
        assert!(Instant::now() < deadline, "the peer console never started");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    append(&status, PERMISSION);

    let (head, reply) = wait_for(local_port, "/api/attention", |r| {
        r.as_array().is_some_and(|a| !a.is_empty())
    })
    .await;
    let requests = reply["requests"].as_array().expect("an array");
    assert_eq!(requests.len(), 1, "{reply}");
    let request = &requests[0];
    assert_eq!(request["id"], format!("{PEER_ID}/console:1"), "{reply}");
    assert_eq!(request["console"], format!("{PEER_ID}/1"), "{reply}");
    assert_eq!(request["repo"], format!("{PEER_ID}/{SLUG}"), "{reply}");
    assert_eq!(request["detail"], "permission: Bash", "{reply}");
    assert_eq!(
        header_value(&head, "x-ralphy-unanswered"),
        Some(DEAD_ID),
        "{head}"
    );
}

#[tokio::test]
async fn a_run_snapshot_in_a_registered_repo_is_listed() {
    let dir = tempfile::tempdir().expect("a temp dir");
    prepare_environment(dir.path());
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("create the repo dir");
    let port = serve(&home, None, AuthState::localhost()).await;
    let (mut presence, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .expect("connect the presence socket");

    // The run is this test: the reader keeps a document whose pid lives.
    let mut snapshot = ralphy_run_snapshot::RunSnapshot {
        runid: "01RUNWAITS".to_string(),
        pid: std::process::id(),
        exec_agent: "codex".to_string(),
        ..ralphy_run_snapshot::RunSnapshot::default()
    };
    snapshot.phase.state = "executing".to_string();
    snapshot.phase.active = Some(71);
    snapshot.phase.agent = Some(ralphy_run_snapshot::AgentBlock {
        state: "waiting".to_string(),
        since: "2026-10-28T11:00:00-03:00".to_string(),
        detail: Some("permission: Bash".to_string()),
    });
    ralphy_run_snapshot::write_atomic(&home, &snapshot).expect("write the run snapshot");
    let pushed = next_push(&mut presence, "attention.dirty").await;
    golden::check("attention.dirty", pushed, &[]);

    let (_, reply) = wait_for(port, "/api/attention", |r| {
        r.as_array().is_some_and(|a| !a.is_empty())
    })
    .await;
    let requests = reply["requests"].as_array().expect("an array");
    assert_eq!(requests.len(), 1, "{reply}");
    let request = &requests[0];
    assert_eq!(request["id"], "run:01RUNWAITS", "{reply}");
    assert_eq!(request["kind"], "run-waiting", "{reply}");
    assert_eq!(request["run"], "01RUNWAITS", "{reply}");
    assert_eq!(request["repo"], SLUG, "{reply}");
    assert_eq!(request["issue"], "71", "{reply}");
    assert_eq!(request["detail"], "permission: Bash", "{reply}");

    std::fs::remove_file(ralphy_run_snapshot::snapshot_path(&home, "01RUNWAITS"))
        .expect("remove the run snapshot");
    let (_, reply) = wait_for(port, "/api/attention", |r| {
        r.as_array().is_some_and(|a| a.is_empty())
    })
    .await;
    assert_eq!(reply["requests"], serde_json::json!([]), "{reply}");
}
