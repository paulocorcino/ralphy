//! End-to-end local-fleet PTY proxy coverage for issue #351.
//!
//! `fleet_console` (issue #352) shares this binary: both set the same two
//! process-wide env values, once, through [`prepare_environment`].

mod fleet_console;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::auth::{AuthPolicy, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity::Identity;
use ralphy_daemon::peer::{self, PeerDescriptor, PEER_PROTOCOL_VERSION};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::WebSocketStream;

type Ws = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const PEER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const DEAD_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const SLUG: &str = "owner/shared";
const ENVIRONMENT: &str = "WSL: Ubuntu-22.04";

struct Daemon {
    port: u16,
    task: tokio::task::JoinHandle<()>,
    shutdown: tokio::sync::watch::Sender<bool>,
}

fn identity(id: &str, name: &str) -> Identity {
    Identity {
        id: id.parse().unwrap(),
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
        environment: ENVIRONMENT.to_string(),
        os: String::new(),
        token: "peer-token".to_string(),
        protocol_version: PEER_PROTOCOL_VERSION,
        tunnel: None,
        nudge: None,
    }
}

fn save_registry(path: &Path, repo: &Path) {
    let mut store = registry::RegistryStore::default();
    store.upsert(SLUG, &repo.to_string_lossy());
    registry::save_to(&store, path).unwrap();
}

fn prepare_environment() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var("WSL_DISTRO_NAME", "Ubuntu-22.04");
        std::env::set_var(
            "RALPHY_DAEMON_AGENT_OVERRIDE",
            env!("CARGO_BIN_EXE_session_test_child"),
        );
    });
}

async fn serve(identity: Identity, registry_path: PathBuf, auth: Arc<AuthState>) -> Daemon {
    prepare_environment();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (shutdown, rx) = tokio::sync::watch::channel(false);
    let app = router(
        Some(identity),
        registry_path,
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        auth,
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Daemon {
        port,
        task,
        shutdown,
    }
}

fn terminal(data: &[u8]) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Terminal {
            session: 0,
            data: data.to_vec(),
        })
        .into(),
    )
}

fn resize(rows: u16, cols: u16) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id: 0,
            verb: "resize".to_string(),
            payload: serde_json::json!({ "rows": rows, "cols": cols }),
        }))
        .into(),
    )
}

fn session_url(port: u16, suffix: &str) -> String {
    format!("ws://127.0.0.1:{port}/ws/session?{suffix}")
}

fn peer_repo() -> String {
    format!("{PEER_ID}/{SLUG}")
}

async fn launch(port: u16) -> Ws {
    let repo = peer_repo().replace('/', "%2F");
    tokio_tungstenite::connect_async(session_url(port, &format!("repo={repo}&agent=claude")))
        .await
        .unwrap()
        .0
}

async fn launch_local(port: u16) -> Ws {
    tokio_tungstenite::connect_async(session_url(port, "repo=owner%2Fshared&agent=claude"))
        .await
        .unwrap()
        .0
}

/// Open a NEW launch the daemon must refuse, and return the refusal's payload:
/// the upgrade succeeds, ONE `session-end` frame says `refused`, the stream ends.
async fn refused(url: &str) -> serde_json::Value {
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("a refused NEW launch still upgrades, so the browser can read why");
    let mut payload = None;
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(msg) = ws.next().await {
            let bytes = match msg {
                Ok(Message::Binary(b)) => b,
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            };
            match protocol::decode(&bytes).expect("a well-formed frame") {
                Frame::Command(cmd) => {
                    assert_eq!(
                        cmd.verb, "session-end",
                        "the only frame is the end: {cmd:?}"
                    );
                    assert_eq!(cmd.payload["reason"], "refused", "{cmd:?}");
                    assert!(payload.is_none(), "exactly one refusal frame");
                    payload = Some(cmd.payload);
                }
                other => panic!("a refusal carries no other frame; got {other:?}"),
            }
        }
    })
    .await
    .expect("the daemon closes a refused launch");
    payload.expect("a session-end frame")
}

async fn attach(port: u16, id: u64) -> Ws {
    let repo = peer_repo().replace('/', "%2F");
    let url = session_url(port, &format!("id={id}&repo={repo}"));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match tokio_tungstenite::connect_async(&url).await {
                Ok((ws, _)) => return ws,
                Err(tungstenite::Error::Http(response))
                    if response.status() == axum::http::StatusCode::CONFLICT =>
                {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(error) => panic!("reattach failed: {error}"),
            }
        }
    })
    .await
    .expect("reattach timed out")
}

#[derive(Default)]
struct Seen {
    terminal: String,
    open: Option<serde_json::Value>,
    end: Option<serde_json::Value>,
    first_frame: Option<String>,
}

async fn read_until(ws: &mut Ws, needle: &str, answer_cursor: bool) -> Seen {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut seen = Seen::default();
        while let Some(message) = ws.next().await {
            match message.unwrap() {
                Message::Binary(bytes) => match protocol::decode(&bytes) {
                    Ok(Frame::Terminal { data, .. }) => {
                        seen.first_frame
                            .get_or_insert_with(|| "terminal".to_string());
                        if answer_cursor
                            && data
                                .windows(CURSOR_POSITION_REQUEST.len())
                                .any(|window| window == CURSOR_POSITION_REQUEST)
                        {
                            ws.send(terminal(CURSOR_POSITION_REPLY)).await.unwrap();
                        }
                        seen.terminal.push_str(&String::from_utf8_lossy(&data));
                        let normalized = seen.terminal.replace("\r\n", "");
                        if normalized.contains(needle) {
                            return seen;
                        }
                    }
                    Ok(Frame::Command(command)) if command.verb == "session-open" => {
                        seen.first_frame.get_or_insert_with(|| command.verb.clone());
                        seen.open = Some(command.payload);
                    }
                    Ok(Frame::Command(command)) if command.verb == "session-end" => {
                        seen.end = Some(command.payload);
                        if needle == "session-end" {
                            return seen;
                        }
                    }
                    _ => {}
                },
                Message::Close(_) => return seen,
                _ => {}
            }
        }
        seen
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}"))
}

async fn http_request(port: u16, method: &str, path: &str, bearer: Option<&str>) -> (u16, String) {
    let (status, _, body) = http_exchange(port, method, path, bearer).await;
    (status, body)
}

/// The status, the header block and the body.
async fn http_exchange(
    port: u16,
    method: &str,
    path: &str,
    bearer: Option<&str>,
) -> (u16, String, String) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let auth = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or_default();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (status, head.to_string(), body.to_string())
}

/// The value of header `name` in a header block from `http_exchange`.
fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

async fn fixture() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    tempfile::TempDir,
    tempfile::TempDir,
    Daemon,
    Daemon,
) {
    let peer_store = tempfile::tempdir().unwrap();
    let peer_repo = tempfile::tempdir().unwrap();
    let peer_registry = peer_store.path().join("repos.toml");
    save_registry(&peer_registry, peer_repo.path());
    let peer = serve(
        identity(PEER_ID, "peer"),
        peer_registry,
        AuthState::fixed(
            AuthPolicy::Bearer("peer-token".to_string()),
            SessionEpoch::in_memory_detached(),
        ),
    )
    .await;

    let local_store = tempfile::tempdir().unwrap();
    let local_repo = tempfile::tempdir().unwrap();
    let local_registry = local_store.path().join("repos.toml");
    save_registry(&local_registry, local_repo.path());
    peer::write_descriptor(local_store.path(), &descriptor(PEER_ID, peer.port)).unwrap();
    let local = serve(
        identity(LOCAL_ID, "local"),
        local_registry,
        AuthState::localhost(),
    )
    .await;
    peer::write_descriptor(peer_store.path(), &descriptor(LOCAL_ID, local.port)).unwrap();
    (peer_store, peer_repo, local_store, local_repo, peer, local)
}

/// Whether the child's `CWD:` line names `dir`, in the spelling the test
/// gave OR the one the OS resolved — `/private/var/…` for a macOS temp dir
/// spelled `/var/…` (the `\\?\` verbatim prefix a Windows canonical path
/// carries is dropped, the child never prints it).
fn names_cwd(terminal: &str, dir: &std::path::Path) -> bool {
    let raw = format!("CWD:{}", dir.display());
    let canonical = dir
        .canonicalize()
        .map(|c| c.display().to_string())
        .unwrap_or_default();
    let canonical = format!("CWD:{}", canonical.trim_start_matches("\\\\?\\"));
    terminal.contains(&raw) || terminal.contains(&canonical)
}

#[tokio::test]
async fn peer_launch_stream_resize_and_interrupt_are_native() {
    let (_peer_store, peer_repo, _local_store, local_repo, peer, local) = fixture().await;
    let mut ws = launch(local.port).await;
    ws.send(terminal(b"peer-input\r")).await.unwrap();
    let seen = read_until(&mut ws, "GOT:peer-input", true).await;
    let open = seen
        .open
        .expect("session-open must precede terminal output");
    assert_eq!(seen.first_frame.as_deref(), Some("session-open"));
    assert_eq!(open["daemon_id"], PEER_ID);
    assert_eq!(open["environment"], ENVIRONMENT);
    assert!(
        names_cwd(&seen.terminal, peer_repo.path()),
        "peer cwd missing: {}",
        seen.terminal
    );
    assert!(
        !names_cwd(&seen.terminal, local_repo.path()),
        "local collision spawned: {}",
        seen.terminal
    );

    ws.send(resize(40, 100)).await.unwrap();
    let resized = read_until(&mut ws, "SIZE 100x40", false).await;
    assert!(resized.terminal.replace("\r\n", "").contains("SIZE 100x40"));

    let mut interrupted = launch(local.port).await;
    let opened = read_until(&mut interrupted, "READY", true).await;
    let interrupted_id = opened.open.unwrap()["session"].as_u64().unwrap();
    interrupted.send(terminal(&[0x03])).await.unwrap();
    let ended = read_until(&mut interrupted, "session-end", false).await;
    let end = ended.end.unwrap();
    assert!(
        ended
            .terminal
            .replace("\r\n", "")
            .contains("INTERRUPTED:ETX"),
        "native child did not observe ETX before exit: {}",
        ended.terminal
    );
    assert_eq!(end["reason"], "child-exited");
    assert_eq!(end["daemon_id"], PEER_ID);
    assert_eq!(end["environment"], ENVIRONMENT);
    assert_eq!(ended.open, None);

    let (_, peer_body) = http_request(peer.port, "GET", "/api/sessions", Some("peer-token")).await;
    let peer_rows: serde_json::Value = serde_json::from_str(&peer_body).unwrap();
    assert!(
        peer_rows
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["id"] != interrupted_id),
        "interrupted session remained listed: {peer_rows}"
    );
}

#[tokio::test]
async fn browser_and_proxy_drops_reattach_to_peer_scrollback() {
    let (_peer_store, _peer_repo, local_store, _local_repo, peer, local) = fixture().await;
    let mut ws = launch(local.port).await;
    ws.send(terminal(b"marker-alpha\r")).await.unwrap();
    let first = read_until(&mut ws, "GOT:marker-alpha", true).await;
    let id = first.open.unwrap()["session"].as_u64().unwrap();
    ws.close(None).await.unwrap();
    drop(ws);

    let (_, listed) = http_request(local.port, "GET", "/api/sessions", None).await;
    let rows: serde_json::Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], id);
    assert_eq!(rows[0]["repo"], peer_repo());
    assert_eq!(rows[0]["daemon_id"], PEER_ID);

    let mut ws = attach(local.port, id).await;
    assert!(read_until(&mut ws, "GOT:marker-alpha", false)
        .await
        .terminal
        .replace("\r\n", "")
        .contains("GOT:marker-alpha"));
    ws.send(terminal(b"marker-beta\r")).await.unwrap();
    assert!(read_until(&mut ws, "GOT:marker-beta", false)
        .await
        .terminal
        .replace("\r\n", "")
        .contains("GOT:marker-beta"));
    ws.send(terminal(b"proxy-survivor\r")).await.unwrap();
    read_until(&mut ws, "GOT:proxy-survivor", false).await;

    local.shutdown.send(true).unwrap();
    local.task.abort();
    let (_, peer_listed) =
        http_request(peer.port, "GET", "/api/sessions", Some("peer-token")).await;
    let peer_rows: serde_json::Value = serde_json::from_str(&peer_listed).unwrap();
    assert_eq!(peer_rows[0]["id"], id);

    let replacement = serve(
        identity(LOCAL_ID, "local"),
        local_store.path().join("repos.toml"),
        AuthState::localhost(),
    )
    .await;
    let mut restored = attach(replacement.port, id).await;
    assert!(read_until(&mut restored, "GOT:proxy-survivor", false)
        .await
        .terminal
        .replace("\r\n", "")
        .contains("GOT:proxy-survivor"));
}

#[tokio::test]
async fn federated_list_and_close_keep_colliding_ids_distinct() {
    let (_peer_store, _peer_repo, _local_store, _local_repo, _peer, local) = fixture().await;
    let mut local_ws = launch_local(local.port).await;
    let local_open = read_until(&mut local_ws, "READY", true).await;
    let local_id = local_open.open.unwrap()["session"].as_u64().unwrap();
    let mut peer_ws = launch(local.port).await;
    let peer_open = read_until(&mut peer_ws, "READY", true).await;
    let peer_id = peer_open.open.unwrap()["session"].as_u64().unwrap();
    assert_eq!(
        local_id, peer_id,
        "fixture must create the numeric collision"
    );

    let (_, body) = http_request(local.port, "GET", "/api/sessions", None).await;
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2, "got {rows}");
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == local_id && row["repo"] == SLUG && row["daemon_id"] == LOCAL_ID));
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == peer_id
            && row["repo"] == peer_repo()
            && row["daemon_id"] == PEER_ID));

    let repo = peer_repo().replace('/', "%2F");
    let (status, _) = http_request(
        local.port,
        "POST",
        &format!("/api/sessions/close?id={peer_id}&repo={repo}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let (_, body) = http_request(local.port, "GET", "/api/sessions", None).await;
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1, "got {rows}");
    assert_eq!(rows[0]["id"], local_id);
    assert_eq!(rows[0]["repo"], SLUG);
    assert_eq!(rows[0]["daemon_id"], LOCAL_ID);
}

/// A peer that does not answer is NAMED by the list, not left out of it: the
/// page reads "no sessions on that peer" otherwise, and relaunches consoles
/// that still run there (a second PTY on a peer without the record join).
#[tokio::test]
async fn the_session_list_names_the_peers_that_did_not_answer() {
    let (_peer_store, _peer_repo, local_store, _local_repo, _peer, local) = fixture().await;
    let mut peer_ws = launch(local.port).await;
    let peer_open = read_until(&mut peer_ws, "READY", true).await;
    let peer_id = peer_open.open.unwrap()["session"].as_u64().unwrap();

    let (status, head, _) = http_exchange(local.port, "GET", "/api/sessions", None).await;
    assert_eq!(status, 200);
    assert_eq!(
        header_value(&head, "x-ralphy-unanswered"),
        None,
        "every peer answered: {head}"
    );

    let closed_port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    peer::write_descriptor(local_store.path(), &descriptor(DEAD_ID, closed_port)).unwrap();
    let (status, head, body) = http_exchange(local.port, "GET", "/api/sessions", None).await;
    assert_eq!(status, 200);
    assert_eq!(
        header_value(&head, "x-ralphy-unanswered"),
        Some(DEAD_ID),
        "{head}"
    );
    // The peer that answered is still listed.
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == peer_id && row["daemon_id"] == PEER_ID),
        "got {rows}"
    );
}

#[tokio::test]
async fn peer_takeover_and_shutdown_end_events_keep_owner_identity() {
    let (_peer_store, _peer_repo, _local_store, _local_repo, peer, local) = fixture().await;
    let mut incumbent = launch(local.port).await;
    let opened = read_until(&mut incumbent, "READY", true).await;
    let id = opened.open.unwrap()["session"].as_u64().unwrap();
    let repo = peer_repo().replace('/', "%2F");
    let (mut taker, _) = tokio_tungstenite::connect_async(session_url(
        local.port,
        &format!("id={id}&repo={repo}&takeover=1"),
    ))
    .await
    .unwrap();
    let takeover = read_until(&mut incumbent, "session-end", false)
        .await
        .end
        .unwrap();
    assert_eq!(takeover["reason"], "taken-over");
    assert_eq!(takeover["daemon_id"], PEER_ID);
    assert_eq!(takeover["environment"], ENVIRONMENT);
    let taker_open = read_until(&mut taker, "READY", false).await.open.unwrap();
    assert_eq!(taker_open["daemon_id"], PEER_ID);

    peer.shutdown.send(true).unwrap();
    let shutdown = read_until(&mut taker, "session-end", false)
        .await
        .end
        .unwrap();
    assert_eq!(shutdown["reason"], "daemon-shutdown");
    assert_eq!(shutdown["daemon_id"], PEER_ID);
    assert_eq!(shutdown["environment"], ENVIRONMENT);
}

#[tokio::test]
async fn unreachable_peer_is_an_environment_diagnosis() {
    let local_store = tempfile::tempdir().unwrap();
    let local_repo = tempfile::tempdir().unwrap();
    let registry = local_store.path().join("repos.toml");
    save_registry(&registry, local_repo.path());
    let closed_port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    peer::write_descriptor(local_store.path(), &descriptor(DEAD_ID, closed_port)).unwrap();
    let local = serve(
        identity(LOCAL_ID, "local"),
        registry,
        AuthState::localhost(),
    )
    .await;

    let repo = format!("{DEAD_ID}/{SLUG}").replace('/', "%2F");
    let refusal = refused(&session_url(
        local.port,
        &format!("repo={repo}&agent=claude"),
    ))
    .await;
    let body = refusal["message"].as_str().unwrap_or_default();
    assert!(body.contains(ENVIRONMENT), "got {body}");
    assert!(!body.contains("serde"), "got {body}");
    assert!(!body.contains("decod"), "got {body}");

    // A reattach keeps the HTTP status: the browser counts it as a failed open.
    let error =
        tokio_tungstenite::connect_async(session_url(local.port, &format!("id=7&repo={repo}")))
            .await
            .unwrap_err();
    let tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP refusal on a reattach, got {error}");
    };
    assert_eq!(response.status(), axum::http::StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn incompatible_peer_is_an_environment_diagnosis() {
    let local_store = tempfile::tempdir().unwrap();
    let local_repo = tempfile::tempdir().unwrap();
    let registry = local_store.path().join("repos.toml");
    save_registry(&registry, local_repo.path());
    let mut incompatible = descriptor(DEAD_ID, 1);
    incompatible.protocol_version = PEER_PROTOCOL_VERSION - 1;
    peer::write_descriptor(local_store.path(), &incompatible).unwrap();
    let local = serve(
        identity(LOCAL_ID, "local"),
        registry,
        AuthState::localhost(),
    )
    .await;

    let repo = format!("{DEAD_ID}/{SLUG}").replace('/', "%2F");
    let refusal = refused(&session_url(
        local.port,
        &format!("repo={repo}&agent=claude"),
    ))
    .await;
    let body = refusal["message"].as_str().unwrap_or_default();
    assert!(body.contains(ENVIRONMENT), "got {body}");
    assert!(body.contains("Upgrade the older Ralphy."), "got {body}");
    assert!(
        body.contains(&format!("protocol {}", PEER_PROTOCOL_VERSION - 1)),
        "got {body}"
    );
    assert!(
        body.contains(&format!("not {PEER_PROTOCOL_VERSION}.")),
        "got {body}"
    );
}

#[tokio::test]
async fn an_old_peer_that_refuses_over_http_is_shown_as_a_refusal() {
    // A peer that predates the refusal frame answers a refused launch with an
    // HTTP status before the upgrade. This stand-in passes the handshake and
    // refuses every session that way.
    const OLD_REFUSAL: &str = "gemini: no owned configuration root in this repo";
    let old_peer = axum::Router::new()
        .route(
            "/api/peer/hello",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "daemon_id": DEAD_ID,
                    "protocol_version": PEER_PROTOCOL_VERSION,
                }))
            }),
        )
        .route(
            "/ws/session",
            axum::routing::get(|| async { (axum::http::StatusCode::BAD_REQUEST, OLD_REFUSAL) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let old_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, old_peer).await.unwrap();
    });

    let local_store = tempfile::tempdir().unwrap();
    let local_repo = tempfile::tempdir().unwrap();
    let registry = local_store.path().join("repos.toml");
    save_registry(&registry, local_repo.path());
    peer::write_descriptor(local_store.path(), &descriptor(DEAD_ID, old_port)).unwrap();
    let local = serve(
        identity(LOCAL_ID, "local"),
        registry,
        AuthState::localhost(),
    )
    .await;

    let repo = format!("{DEAD_ID}/{SLUG}").replace('/', "%2F");
    let refusal = refused(&session_url(
        local.port,
        &format!("repo={repo}&agent=gemini"),
    ))
    .await;
    assert_eq!(
        refusal["message"], OLD_REFUSAL,
        "the peer's own words reach the console"
    );
}
