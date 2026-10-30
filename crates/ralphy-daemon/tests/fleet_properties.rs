//! `file.stat` and `tree.size` of a project that lives on a peer are answered by
//! the peer: the local daemon relays the verb, with the repo named by the peer's
//! own slug, and passes the reply on as the peer wrote it (ADR-0036 amendment
//! 2026-10-15, ADR-0052). Both daemons are real; the local one holds a project
//! with the same slug and a file of the same name, so a reply that came from the
//! local disk would differ.

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
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

const A_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const B_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const SLUG: &str = "owner/shared";

fn identity(id: &str, name: &str) -> Identity {
    Identity {
        id: id.parse().expect("the test id parses"),
        name: name.to_string(),
        avatar: "🐙".to_string(),
    }
}

fn save_registry(path: &Path, repo: &Path) {
    let mut store = registry::RegistryStore::default();
    store.upsert(SLUG, &repo.to_string_lossy());
    registry::save_to(&store, path).expect("the registry is saved");
}

async fn serve(identity: Identity, registry_path: PathBuf, auth: Arc<AuthState>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the test port binds");
    let port = listener
        .local_addr()
        .expect("the test port has an address")
        .port();
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
    tokio::spawn(async move {
        let _shutdown = tx;
        axum::serve(listener, app)
            .await
            .expect("the test server runs");
    });
    port
}

fn descriptor(port: u16) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: B_ID.to_string(),
        name: "peer".to_string(),
        avatar: "🐙".to_string(),
        address: "127.0.0.1".to_string(),
        port,
        environment: "WSL: Ubuntu-22.04".to_string(),
        os: String::new(),
        token: "peer-tok".to_string(),
        protocol_version: PEER_PROTOCOL_VERSION,
        tunnel: None,
        nudge: None,
    }
}

async fn ask(port: u16, verb: &str, payload: Value) -> Value {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws/command"))
        .await
        .expect("the websocket connects");
    ws.send(Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id: 1,
            verb: verb.to_string(),
            payload,
        }))
        .into(),
    ))
    .await
    .expect("the frame is sent");
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = ws.next().await {
            if let Message::Binary(bytes) = message.expect("the websocket frame arrives") {
                if let Ok(Frame::Command(reply)) = protocol::decode(&bytes) {
                    if reply.id == 1 {
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

/// A local daemon that relays to a peer. The peer's project holds `note.txt`
/// (9 bytes) and `sub/` with one file of 3 bytes; the local project of the same
/// slug holds a `note.txt` of 10 bytes and a `sub/` with one file of 7 bytes.
async fn fleet() -> u16 {
    let peer_store = tempfile::tempdir().expect("the temp dir is created");
    let peer_repo = tempfile::tempdir().expect("the temp dir is created");
    std::fs::write(peer_repo.path().join("note.txt"), "peer-side")
        .expect("the test file is written");
    std::fs::create_dir(peer_repo.path().join("sub")).expect("the test directory is created");
    std::fs::write(peer_repo.path().join("sub/x"), "abc").expect("the test file is written");
    let peer_registry = peer_store.path().join("repos.toml");
    save_registry(&peer_registry, peer_repo.path());
    let peer_port = serve(
        identity(B_ID, "peer"),
        peer_registry,
        AuthState::fixed(
            AuthPolicy::Bearer("peer-tok".to_string()),
            SessionEpoch::in_memory_detached(),
        ),
    )
    .await;

    let local_store = tempfile::tempdir().expect("the temp dir is created");
    let local_repo = tempfile::tempdir().expect("the temp dir is created");
    std::fs::write(local_repo.path().join("note.txt"), "local-side")
        .expect("the test file is written");
    std::fs::create_dir(local_repo.path().join("sub")).expect("the test directory is created");
    std::fs::write(local_repo.path().join("sub/x"), "abcdefg").expect("the test file is written");
    let local_registry = local_store.path().join("repos.toml");
    save_registry(&local_registry, local_repo.path());
    peer::write_descriptor(local_store.path(), &descriptor(peer_port))
        .expect("the peer descriptor is written");
    let port = serve(
        identity(A_ID, "local"),
        local_registry,
        AuthState::localhost(),
    )
    .await;
    // The stores must outlive the daemons, which read them on every command; the
    // OS reclaims them when the test process exits.
    std::mem::forget((peer_store, peer_repo, local_store, local_repo));
    port
}

#[tokio::test]
async fn the_peer_answers_the_facts_of_its_own_file() {
    let port = fleet().await;
    let peer = format!("{B_ID}/{SLUG}");

    let on_peer = ask(
        port,
        "file.stat",
        json!({ "repo": peer, "path": "note.txt" }),
    )
    .await;
    let on_local = ask(
        port,
        "file.stat",
        json!({ "repo": SLUG, "path": "note.txt" }),
    )
    .await;

    assert_eq!(
        (&on_peer["status"], &on_peer["size"]),
        (&json!("ok"), &json!(9)),
        "{on_peer}"
    );
    // The control: the same name on the local disk is 10 bytes.
    assert_eq!(on_local["size"], 10, "{on_local}");
}

#[tokio::test]
async fn the_peer_counts_its_own_folder() {
    let port = fleet().await;
    let peer = format!("{B_ID}/{SLUG}");

    let on_peer = ask(port, "tree.size", json!({ "repo": peer, "path": "sub" })).await;
    let on_local = ask(port, "tree.size", json!({ "repo": SLUG, "path": "sub" })).await;

    assert_eq!(
        (&on_peer["bytes"], &on_peer["files"]),
        (&json!(3), &json!(1)),
        "{on_peer}"
    );
    assert_eq!(on_local["bytes"], 7, "{on_local}");
}

#[tokio::test]
async fn a_refusal_of_the_peer_reaches_the_page_as_the_peer_wrote_it() {
    let port = fleet().await;
    let peer = format!("{B_ID}/{SLUG}");
    let not_found = json!({ "status": "error", "reason": "not found" });

    let stat = ask(
        port,
        "file.stat",
        json!({ "repo": peer, "path": "nope.txt" }),
    )
    .await;
    let size = ask(
        port,
        "tree.size",
        json!({ "repo": peer, "path": "note.txt" }),
    )
    .await;

    assert_eq!(stat, not_found);
    // A file has a stat, not a size.
    assert_eq!(size, not_found);
}
