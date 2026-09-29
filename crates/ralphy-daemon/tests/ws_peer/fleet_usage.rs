//! Fleet usage stays attributable and states missing peer contributions.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ralphy_daemon::auth::{AuthPolicy, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity::Identity;
use ralphy_daemon::peer::{self, PeerDescriptor, PEER_PROTOCOL_VERSION};
use ralphy_daemon::{router, StorePaths};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const PEER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const OLD_PEER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const ENVIRONMENT: &str = "WSL: Ubuntu-22.04";
const OLD_ENVIRONMENT: &str = "WSL: Debian";

struct Daemon {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

fn identity(id: &str, name: &str) -> Identity {
    Identity {
        id: id.parse().unwrap(),
        name: name.to_string(),
        avatar: "🐙".to_string(),
    }
}

fn descriptor(id: &str, port: u16, token: &str) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: id.to_string(),
        name: "peer".to_string(),
        avatar: "🐙".to_string(),
        address: "127.0.0.1".to_string(),
        port,
        environment: ENVIRONMENT.to_string(),
        token: token.to_string(),
        protocol_version: PEER_PROTOCOL_VERSION,
        nudge: None,
    }
}

fn write_usage(dir: &std::path::Path, session_id: &str) {
    std::fs::write(
        dir.join("fixture.jsonl"),
        serde_json::to_string(&serde_json::json!({
            "ts": "2026-07-29T12:00:00Z",
            "session_id": session_id,
            "agent": "codex",
            "tokens": 352
        }))
        .unwrap(),
    )
    .unwrap();
}

async fn serve(
    identity: Identity,
    registry_path: PathBuf,
    usage_dir: PathBuf,
    auth: Arc<AuthState>,
) -> Daemon {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (shutdown, rx) = tokio::sync::watch::channel(false);
    let app = router(
        Some(identity),
        registry_path,
        usage_dir,
        StorePaths::default(),
        Instant::now(),
        rx,
        auth,
    );
    let task = tokio::spawn(async move {
        let _shutdown = shutdown;
        axum::serve(listener, app).await.unwrap();
    });
    Daemon { port, task }
}

async fn usage(port: u16) -> serde_json::Value {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(
                format!(
                    "GET /api/usage HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "got {response}");
        let body = response.split_once("\r\n\r\n").unwrap().1;
        serde_json::from_str(body).unwrap()
    })
    .await
    .expect("usage request timed out")
}

#[tokio::test]
async fn usage_federates_and_names_missing_contributions() {
    let peer_store = tempfile::tempdir().unwrap();
    let peer_usage = tempfile::tempdir().unwrap();
    write_usage(peer_usage.path(), "peer-usage-352");
    let peer = serve(
        identity(PEER_ID, "peer"),
        peer_store.path().join("repos.toml"),
        peer_usage.path().to_path_buf(),
        AuthState::fixed(
            AuthPolicy::Bearer("peer-token".to_string()),
            SessionEpoch::in_memory_detached(),
        ),
    )
    .await;

    let local_store = tempfile::tempdir().unwrap();
    let local_usage = tempfile::tempdir().unwrap();
    write_usage(local_usage.path(), "local-usage-352");
    peer::write_descriptor(
        local_store.path(),
        &descriptor(PEER_ID, peer.port, "peer-token"),
    )
    .unwrap();
    let local = serve(
        identity(LOCAL_ID, "local"),
        local_store.path().join("repos.toml"),
        local_usage.path().to_path_buf(),
        AuthState::localhost(),
    )
    .await;
    peer::write_descriptor(
        peer_store.path(),
        &descriptor(LOCAL_ID, local.port, "unused"),
    )
    .unwrap();

    let live = usage(local.port).await;
    let sessions = live["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|record| record["session_id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(sessions, ["local-usage-352", "peer-usage-352"]);
    assert_eq!(live["records"][0]["daemon_id"], LOCAL_ID);
    assert_eq!(live["records"][1]["daemon_id"], PEER_ID);
    assert_eq!(live["missing"], serde_json::json!([]));

    let mut old_peer = descriptor(OLD_PEER_ID, 1, "unused");
    old_peer.environment = OLD_ENVIRONMENT.to_string();
    old_peer.protocol_version = PEER_PROTOCOL_VERSION - 1;
    peer::write_descriptor(local_store.path(), &old_peer).unwrap();
    let incompatible = usage(local.port).await;
    assert_eq!(incompatible["records"].as_array().unwrap().len(), 2);
    assert_eq!(incompatible["missing"][0]["daemon_id"], OLD_PEER_ID);
    assert_eq!(incompatible["missing"][0]["environment"], OLD_ENVIRONMENT);
    assert!(
        incompatible["missing"][0]["why"]
            .as_str()
            .unwrap()
            .contains("speaks peer protocol"),
        "got {incompatible}"
    );

    peer.task.abort();
    let _ = peer.task.await;
    let partial = usage(local.port).await;
    assert_eq!(partial["records"].as_array().unwrap().len(), 1);
    assert_eq!(partial["records"][0]["session_id"], "local-usage-352");
    let missing_peer = partial["missing"]
        .as_array()
        .unwrap()
        .iter()
        .find(|missing| missing["daemon_id"] == PEER_ID)
        .unwrap();
    assert_eq!(missing_peer["environment"], ENVIRONMENT);
    assert!(
        missing_peer["why"].as_str().unwrap().contains("connecting"),
        "got {partial}"
    );
}
