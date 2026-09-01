//! Free-console hosting exception coverage for local-fleet issue #352.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::auth::{AuthPolicy, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity::Identity;
use ralphy_daemon::peer::{self, NudgeSpec, PeerDescriptor, PEER_PROTOCOL_VERSION};
use ralphy_daemon::protocol::{self, Frame};
use ralphy_daemon::{registry, router};
use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const PEER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
const SLUG: &str = "owner/shared";
const PEER_ENVIRONMENT: &str = "WSL: Ubuntu-22.04";

struct Daemon {
    port: u16,
    task: tokio::task::JoinHandle<()>,
    _shutdown: tokio::sync::watch::Sender<bool>,
}

fn identity(id: &str, name: &str) -> Identity {
    Identity {
        id: id.parse().unwrap(),
        name: name.to_string(),
        avatar: "🐙".to_string(),
    }
}

fn descriptor(port: u16) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: PEER_ID.to_string(),
        name: "peer".to_string(),
        avatar: "🐙".to_string(),
        address: "127.0.0.1".to_string(),
        port,
        environment: PEER_ENVIRONMENT.to_string(),
        token: "peer-token".to_string(),
        protocol_version: PEER_PROTOCOL_VERSION,
        nudge: Some(NudgeSpec {
            distro: "Ubuntu-22.04".to_string(),
            unit: "ralphy-daemon.service".to_string(),
        }),
    }
}

fn save_registry(path: &Path, repo: &Path) {
    let mut store = registry::RegistryStore::default();
    store.upsert(SLUG, &repo.to_string_lossy());
    registry::save_to(&store, path).unwrap();
}

fn prepare_environment() {
    std::env::set_var("WSL_DISTRO_NAME", "Ubuntu-22.04");
    std::env::set_var(
        "RALPHY_DAEMON_AGENT_OVERRIDE",
        env!("CARGO_BIN_EXE_session_test_child"),
    );
}

async fn serve(identity: Identity, registry_path: PathBuf, auth: Arc<AuthState>) -> Daemon {
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
        _shutdown: shutdown,
    }
}

fn peer_repo() -> String {
    format!("{PEER_ID}/{SLUG}")
}

async fn launch(
    port: u16,
    query: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws/session?{query}"))
        .await
        .unwrap()
        .0
}

async fn read_until(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    needle: &str,
) -> (String, Option<serde_json::Value>) {
    let mut terminal = String::new();
    let mut open = None;
    let completed = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = ws.next().await {
            match message.unwrap() {
                Message::Binary(bytes) => match protocol::decode(&bytes) {
                    Ok(Frame::Terminal { data, .. }) => {
                        if data
                            .windows(CURSOR_POSITION_REQUEST.len())
                            .any(|window| window == CURSOR_POSITION_REQUEST)
                        {
                            ws.send(Message::Binary(protocol::encode(&Frame::Terminal {
                                session: 0,
                                data: CURSOR_POSITION_REPLY.to_vec(),
                            })))
                            .await
                            .unwrap();
                        }
                        terminal.push_str(&String::from_utf8_lossy(&data));
                        if terminal.replace("\r\n", "").contains(needle) {
                            return true;
                        }
                    }
                    Ok(Frame::Command(command)) if command.verb == "session-open" => {
                        open = Some(command.payload);
                    }
                    Ok(Frame::Command(command))
                        if command.verb == "session-end" && needle == "session-end" =>
                    {
                        return true;
                    }
                    _ => {}
                },
                Message::Close(_) => return true,
                _ => {}
            }
        }
        true
    })
    .await;
    assert!(
        completed.is_ok(),
        "timed out waiting for {needle:?}; open={open:?}; terminal={terminal:?}"
    );
    (terminal, open)
}

async fn send_line(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    line: &str,
) {
    ws.send(Message::Binary(protocol::encode(&Frame::Terminal {
        session: 0,
        data: format!("{line}\r").into_bytes(),
    })))
    .await
    .unwrap();
}

async fn http_json(port: u16, method: &str, path: &str, bearer: Option<&str>) -> serde_json::Value {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let auth = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Content-Length: 0\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let text = String::from_utf8(response).unwrap();
    assert!(text.starts_with("HTTP/1.1 200"), "{text}");
    serde_json::from_str(text.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
async fn peer_free_console_is_local_and_agent_stays_on_the_owner() {
    prepare_environment();
    let peer_store = tempfile::tempdir().unwrap();
    let peer_repo_dir = tempfile::tempdir().unwrap();
    let peer_registry = peer_store.path().join("repos.toml");
    save_registry(&peer_registry, peer_repo_dir.path());
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
    let local_repo_dir = tempfile::tempdir().unwrap();
    let local_registry = local_store.path().join("repos.toml");
    save_registry(&local_registry, local_repo_dir.path());
    peer::write_descriptor(local_store.path(), &descriptor(peer.port)).unwrap();
    let local = serve(
        identity(LOCAL_ID, "local"),
        local_registry,
        AuthState::localhost(),
    )
    .await;

    let encoded_repo = peer_repo().replace('/', "%2F");
    let mut console = launch(local.port, &format!("console=1&repo={encoded_repo}")).await;
    let (_, open) = read_until(&mut console, "READY").await;
    let open = open.expect("free console must announce its effective environment");
    let console_id = open["session"].as_u64().unwrap();
    assert_eq!(open["daemon_id"], LOCAL_ID);
    assert_eq!(open["environment"], PEER_ENVIRONMENT);
    send_line(&mut console, "argv").await;
    let (argv, _) = read_until(&mut console, "ARGV:").await;
    assert!(
        argv.replace("\r\n", "").contains(&format!(
            "ARGV:-d Ubuntu-22.04 --cd {}",
            peer_repo_dir.path().display()
        )),
        "launcher argv did not preserve typed WSL tokens: {argv}"
    );

    let local_rows = http_json(local.port, "GET", "/api/sessions?local=1", None).await;
    assert_eq!(local_rows.as_array().unwrap().len(), 1);
    assert_eq!(local_rows[0]["repo"], peer_repo());
    assert_eq!(local_rows[0]["kind"], "console");
    assert_eq!(local_rows[0]["daemon_id"], LOCAL_ID);
    assert_eq!(local_rows[0]["environment"], PEER_ENVIRONMENT);
    let peer_rows = http_json(
        peer.port,
        "GET",
        "/api/sessions?local=1",
        Some("peer-token"),
    )
    .await;
    assert!(
        peer_rows.as_array().unwrap().is_empty(),
        "peer daemon unexpectedly hosted the free console: {peer_rows}"
    );

    console.close(None).await.unwrap();
    drop(console);
    let mut console = launch(
        local.port,
        &format!("id={console_id}&repo={encoded_repo}&takeover=1"),
    )
    .await;
    let (replayed, reopened) = read_until(&mut console, "ARGV:").await;
    assert!(replayed.contains("-d Ubuntu-22.04 --cd"));
    assert_eq!(reopened.unwrap()["daemon_id"], LOCAL_ID);
    let close = http_json(
        local.port,
        "POST",
        &format!("/api/sessions/close?id={console_id}&repo={encoded_repo}"),
        None,
    )
    .await;
    assert_eq!(close["closed"], true);
    drop(console);

    let mut agent = launch(local.port, &format!("repo={encoded_repo}&agent=claude")).await;
    let (_, agent_open) = read_until(&mut agent, "READY").await;
    let agent_open = agent_open.expect("peer agent must announce its owning daemon");
    assert_eq!(agent_open["daemon_id"], PEER_ID);
    assert_eq!(agent_open["environment"], PEER_ENVIRONMENT);
    send_line(&mut agent, "argv").await;
    let (agent_argv, _) = read_until(&mut agent, "ARGV:").await;
    let argv = agent_argv
        .replace("\r\n", "")
        .split("ARGV:")
        .nth(1)
        .unwrap_or_default()
        .to_string();
    assert!(
        !argv.contains("-d ") && !argv.contains("--cd"),
        "agent session received free-console launcher tokens: {agent_argv}"
    );
    // A peer-hosted Claude console is named like a local one — the naming lives
    // in `spec_for`, which the OWNING daemon runs, so proxying must not lose it.
    assert!(
        argv.starts_with("--name wb-"),
        "a peer-hosted Claude console must still be named: {agent_argv}"
    );

    let local_rows = http_json(local.port, "GET", "/api/sessions?local=1", None).await;
    assert!(
        local_rows.as_array().unwrap().is_empty(),
        "agent session leaked into local manager: {local_rows}"
    );
    let peer_rows = http_json(
        peer.port,
        "GET",
        "/api/sessions?local=1",
        Some("peer-token"),
    )
    .await;
    assert_eq!(peer_rows.as_array().unwrap().len(), 1);
    assert_eq!(peer_rows[0]["kind"], "agent");
    assert_eq!(peer_rows[0]["daemon_id"], PEER_ID);

    local.task.abort();
    peer.task.abort();
}
