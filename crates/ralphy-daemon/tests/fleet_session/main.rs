//! End-to-end local-fleet PTY proxy coverage for issue #351.
//!
//! `fleet_console` (issue #352) shares this binary: both set the same two
//! process-wide env values, once, through [`prepare_environment`].

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

#[path = "../support/page.rs"]
pub mod page;

mod fleet_console;
mod peer_diagnosis;
mod peer_session;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ralphy_daemon::auth::{AuthPolicy, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity::Identity;
use ralphy_daemon::peer::{self, PeerDescriptor, PEER_PROTOCOL_VERSION};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{self, protocol::CloseFrame, Message};

/// A console channel of the page socket. A reattach reads its first message
/// ahead to learn whether the daemon took it, so that message waits here.
struct Ws {
    channel: page::Channel,
    ahead: Option<Message>,
}

impl Ws {
    fn new(channel: page::Channel) -> Ws {
        Ws {
            channel,
            ahead: None,
        }
    }

    async fn send(&mut self, message: Message) -> Result<(), tungstenite::Error> {
        self.channel.send(message).await
    }

    async fn next(&mut self) -> Option<Result<Message, tungstenite::Error>> {
        match self.ahead.take() {
            Some(message) => Some(Ok(message)),
            None => self.channel.next().await,
        }
    }

    async fn close(&mut self, frame: Option<CloseFrame>) -> Result<(), tungstenite::Error> {
        self.channel.close(frame).await
    }
}

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
        id: id.parse().expect("the test id parses"),
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
    registry::save_to(&store, path).expect("the registry is saved");
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the test port binds");
    let port = listener
        .local_addr()
        .expect("the test port has an address")
        .port();
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
        axum::serve(listener, app)
            .await
            .expect("the test server runs");
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

fn peer_repo() -> String {
    format!("{PEER_ID}/{SLUG}")
}

async fn launch(port: u16) -> Ws {
    let fields = serde_json::json!({ "repo": peer_repo(), "agent": "claude" });
    Ws::new(page::console(port, fields).await)
}

async fn launch_local(port: u16) -> Ws {
    let fields = serde_json::json!({ "repo": SLUG, "agent": "claude" });
    Ws::new(page::console(port, fields).await)
}

/// Reattach to session `id` of the peer's repo. The peer may still hold the
/// previous client for a moment: a refused reattach is tried again.
async fn attach(port: u16, id: u64) -> Ws {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let fields = serde_json::json!({ "id": id, "repo": peer_repo() });
            let mut channel = page::console(port, fields).await;
            match channel.next().await {
                Some(Ok(Message::Close(_))) | None => {
                    assert_eq!(
                        channel.closed_reason(),
                        Some("refused"),
                        "a reattach ends only as refused"
                    );
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Some(Ok(first)) => {
                    return Ws {
                        channel,
                        ahead: Some(first),
                    }
                }
                Some(Err(error)) => panic!("reattach failed: {error}"),
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
            match message.expect("the websocket frame arrives") {
                Message::Binary(bytes) => match protocol::decode(&bytes) {
                    Ok(Frame::Terminal { data, .. }) => {
                        seen.first_frame
                            .get_or_insert_with(|| "terminal".to_string());
                        if answer_cursor
                            && data
                                .windows(CURSOR_POSITION_REQUEST.len())
                                .any(|window| window == CURSOR_POSITION_REQUEST)
                        {
                            ws.send(terminal(CURSOR_POSITION_REPLY))
                                .await
                                .expect("the frame is sent");
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
                Message::Text(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
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
        .expect("the test socket connects");
    let auth = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the request is written");
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .await
        .expect("the reply is read");
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
    let peer_store = tempfile::tempdir().expect("the temp dir is created");
    let peer_repo = tempfile::tempdir().expect("the temp dir is created");
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

    let local_store = tempfile::tempdir().expect("the temp dir is created");
    let local_repo = tempfile::tempdir().expect("the temp dir is created");
    let local_registry = local_store.path().join("repos.toml");
    save_registry(&local_registry, local_repo.path());
    peer::write_descriptor(local_store.path(), &descriptor(PEER_ID, peer.port))
        .expect("the peer descriptor is written");
    let local = serve(
        identity(LOCAL_ID, "local"),
        local_registry,
        AuthState::localhost(),
    )
    .await;
    peer::write_descriptor(peer_store.path(), &descriptor(LOCAL_ID, local.port))
        .expect("the peer descriptor is written");
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
