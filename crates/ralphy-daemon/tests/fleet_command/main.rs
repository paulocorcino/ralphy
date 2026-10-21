//! Repo commands proxy through the local daemon and execute on the owning peer.

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
use tokio_tungstenite::tungstenite::Message;

mod spawn_verbs;

const A_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const B_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const DEAD_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const SLUG: &str = "owner/shared";
const ENV: &str = "WSL: Ubuntu-22.04";

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
    auth: Arc<AuthState>,
) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::watch::channel(false);
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
        let _shutdown = tx;
        axum::serve(listener, app).await.unwrap();
    });
    (port, task)
}

fn descriptor(id: &str, port: u16) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: id.to_string(),
        name: "peer".to_string(),
        avatar: "🐙".to_string(),
        address: "127.0.0.1".to_string(),
        port,
        environment: ENV.to_string(),
        os: String::new(),
        token: "peer-tok".to_string(),
        protocol_version: PEER_PROTOCOL_VERSION,
        tunnel: None,
        nudge: None,
    }
}

async fn ask(port: u16, id: u64, verb: &str, payload: serde_json::Value) -> serde_json::Value {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws/command"))
        .await
        .unwrap();
    ws.send(Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id,
            verb: verb.to_string(),
            payload,
        }))
        .into(),
    ))
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = ws.next().await {
            if let Message::Binary(bytes) = message.unwrap() {
                if let Ok(Frame::Command(reply)) = protocol::decode(&bytes) {
                    if reply.id == id {
                        return reply.payload;
                    }
                }
            }
        }
        panic!("command socket closed without a reply");
    })
    .await
    .expect("command reply timed out")
}

async fn ask_all(port: u16, id: u64, verb: &str, payload: serde_json::Value) -> Vec<Command> {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws/command"))
        .await
        .unwrap();
    ws.send(Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id,
            verb: verb.to_string(),
            payload,
        }))
        .into(),
    ))
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        let mut replies = Vec::new();
        while let Some(message) = ws.next().await {
            let Message::Binary(bytes) = message.unwrap() else {
                continue;
            };
            let Ok(Frame::Command(reply)) = protocol::decode(&bytes) else {
                continue;
            };
            if reply.id != id {
                continue;
            }
            let terminal = matches!(
                reply.payload.get("status").and_then(|value| value.as_str()),
                Some("exited" | "error")
            );
            replies.push(reply);
            if terminal {
                return replies;
            }
        }
        panic!("command socket closed without a terminal reply");
    })
    .await
    .expect("command reply timed out")
}

/// Whether the child's `dispatch-cwd:` line names `repo` — compared canonical,
/// since the child prints the cwd the OS resolved (`/private/var/…` for a
/// macOS temp dir spelled `/var/…`).
fn ran_in(text: &str, repo: &std::path::Path) -> bool {
    let want = repo.canonicalize().expect("the repo exists");
    text.lines()
        .filter_map(|l| l.strip_prefix("dispatch-cwd: "))
        .any(|cwd| {
            std::path::Path::new(cwd.trim())
                .canonicalize()
                .is_ok_and(|got| got == want)
        })
}
