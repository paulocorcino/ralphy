//! Peer filesystem nudges are re-stamped with the routed repo ref.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::auth::{AuthPolicy, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity::Identity;
use ralphy_daemon::peer::{self, PeerDescriptor, PEER_PROTOCOL_VERSION};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

const A_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const B_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const SLUG: &str = "owner/shared";

fn identity(id: &str, name: &str) -> Identity {
    Identity {
        id: id.parse().unwrap(),
        name: name.to_string(),
        avatar: "🐙".to_string(),
    }
}

fn save_registry(path: &Path, repo: &Path) {
    let mut store = registry::RegistryStore::default();
    store.upsert(SLUG, &repo.to_string_lossy());
    registry::save_to(&store, path).unwrap();
}

async fn serve(
    identity: Identity,
    registry_path: PathBuf,
    auth: std::sync::Arc<AuthState>,
) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(_tx);
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
    (port, task)
}

async fn send_watch(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    repo: &str,
) {
    ws.send(Message::Binary(protocol::encode(&Frame::Command(
        Command {
            id: 1,
            verb: "watch".to_string(),
            payload: serde_json::json!({ "repo": repo, "path": "" }),
        },
    ))))
    .await
    .unwrap();
}

async fn next_tree_dirty(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    repo: &str,
) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = ws.next().await {
            if let Message::Binary(bytes) = message.unwrap() {
                if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                    if cmd.verb == "tree.dirty" && cmd.payload["repo"] == repo {
                        return cmd.payload;
                    }
                }
            }
        }
        panic!("tree socket closed without a matching dirty frame");
    })
    .await
    .expect("tree dirty timed out")
}

#[tokio::test]
async fn peer_and_local_creates_push_their_own_repo_identity() {
    let peer_store = tempfile::tempdir().unwrap();
    let peer_repo = tempfile::tempdir().unwrap();
    let peer_registry = peer_store.path().join("repos.toml");
    save_registry(&peer_registry, peer_repo.path());
    let (peer_port, _peer_task) = serve(
        identity(B_ID, "peer"),
        peer_registry,
        AuthState::fixed(
            AuthPolicy::Bearer("peer-tok".to_string()),
            SessionEpoch::in_memory_detached(),
        ),
    )
    .await;

    let local_store = tempfile::tempdir().unwrap();
    let local_repo = tempfile::tempdir().unwrap();
    let local_registry = local_store.path().join("repos.toml");
    save_registry(&local_registry, local_repo.path());
    peer::write_descriptor(
        local_store.path(),
        &PeerDescriptor {
            daemon_id: B_ID.to_string(),
            name: "peer".to_string(),
            avatar: "🐙".to_string(),
            address: "127.0.0.1".to_string(),
            port: peer_port,
            environment: "WSL: Ubuntu-22.04".to_string(),
            token: "peer-tok".to_string(),
            protocol_version: PEER_PROTOCOL_VERSION,
            nudge: None,
        },
    )
    .unwrap();
    let (local_port, _local_task) = serve(
        identity(A_ID, "local"),
        local_registry,
        AuthState::localhost(),
    )
    .await;

    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{local_port}/ws/tree"))
            .await
            .unwrap();
    let peer_ref = format!("{B_ID}/{SLUG}");
    send_watch(&mut ws, &peer_ref).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    std::fs::write(peer_repo.path().join("fresh.txt"), "peer").unwrap();
    let peer_dirty = next_tree_dirty(&mut ws, &peer_ref).await;
    assert_eq!(peer_dirty["repo"], peer_ref);
    assert_eq!(peer_dirty["path"], "");

    send_watch(&mut ws, SLUG).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    std::fs::write(local_repo.path().join("local.txt"), "local").unwrap();
    let local_dirty = next_tree_dirty(&mut ws, SLUG).await;
    assert_eq!(local_dirty["repo"], SLUG);
    assert_eq!(local_dirty["path"], "");
}
