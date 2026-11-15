//! Peer filesystem nudges are re-stamped with the routed repo ref.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ralphy_daemon::auth::{AuthPolicy, AuthState};
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::identity::Identity;
use ralphy_daemon::peer::{self, PeerDescriptor, PEER_PROTOCOL_VERSION};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

use crate::page::{self, Channel};

const A_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const B_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const SLUG: &str = "owner/shared";

fn identity(id: &str, name: &str) -> Identity {
    Identity {
        id: id.parse().expect("parse the peer id"),
        name: name.to_owned(),
        avatar: "🐙".to_owned(),
    }
}

fn save_registry(path: &Path, repo: &Path) {
    let mut store = registry::RegistryStore::default();
    store.upsert(SLUG, &repo.to_string_lossy());
    registry::save_to(&store, path).expect("save the registry");
}

async fn serve(
    identity: Identity,
    registry_path: PathBuf,
    auth: std::sync::Arc<AuthState>,
) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let _kept = Box::leak(Box::new(tx));
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
            .expect("serve the test app");
    });
    (port, task)
}

async fn send_watch(ws: &mut Channel, repo: &str) {
    ws.send(Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id: 1,
            verb: "watch".to_owned(),
            payload: serde_json::json!({ "repo": repo, "path": "" }),
        }))
        .into(),
    ))
    .await
    .expect("send the frame");
}

async fn tree_dirty(ws: &mut Channel, repo: &str) -> Option<serde_json::Value> {
    while let Some(message) = ws.next().await {
        if let Message::Binary(bytes) = message.expect("read the next frame") {
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.verb == "tree.dirty"
                    && cmd.payload.get("repo").and_then(serde_json::Value::as_str) == Some(repo)
                {
                    return Some(cmd.payload);
                }
            }
        }
    }
    None
}

/// Writes `file` in `dir` until the watch of `repo` reports it. The watch
/// starts after the `watch` frame, and under the load of the whole suite a
/// single write can come before it (#950).
async fn dirty_after_writes(
    ws: &mut Channel,
    repo: &str,
    dir: &Path,
    file: &str,
) -> serde_json::Value {
    let mut seen = None;
    for round in 0..20 {
        std::fs::write(dir.join(file), format!("{repo} {round}")).expect("write the file");
        if let Ok(found) = tokio::time::timeout(Duration::from_secs(1), tree_dirty(ws, repo)).await
        {
            seen = Some(found.expect("tree channel closed without a matching dirty frame"));
            break;
        }
    }
    seen.expect("tree dirty timed out")
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
            AuthPolicy::Bearer("peer-tok".to_owned()),
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
            daemon_id: B_ID.to_owned(),
            name: "peer".to_owned(),
            avatar: "🐙".to_owned(),
            address: "127.0.0.1".to_owned(),
            port: peer_port,
            environment: "WSL: Ubuntu-22.04".to_owned(),
            os: String::new(),
            token: "peer-tok".to_owned(),
            protocol_version: PEER_PROTOCOL_VERSION,
            tunnel: None,
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

    let mut ws = page::tree(local_port).await;
    let peer_ref = format!("{B_ID}/{SLUG}");
    send_watch(&mut ws, &peer_ref).await;
    let peer_dirty = dirty_after_writes(&mut ws, &peer_ref, peer_repo.path(), "fresh.txt").await;
    assert_eq!(peer_dirty["repo"], peer_ref);
    assert_eq!(peer_dirty["path"], "");

    send_watch(&mut ws, SLUG).await;
    let local_dirty = dirty_after_writes(&mut ws, SLUG, local_repo.path(), "local.txt").await;
    assert_eq!(local_dirty["repo"], SLUG);
    assert_eq!(local_dirty["path"], "");
}
