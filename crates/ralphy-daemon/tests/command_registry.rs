//! The registry verbs on `/ws/command` (ADR-0036 amendment "the registry
//! verbs"): `dir.list` and `project.add` need no `repo`, are relayed to a peer
//! by its `daemon` id, refuse an unknown id, and refuse a bad path before
//! anything spawns. The spawned exe is `command_test_child`, which logs its
//! argv.

#[path = "support/golden.rs"]
mod golden;

use std::path::{Path, PathBuf, MAIN_SEPARATOR};
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

const A_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const B_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const UNKNOWN_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";

fn identity(id: &str) -> Identity {
    Identity {
        id: id.parse().unwrap(),
        name: "daemon".to_string(),
        avatar: "🐙".to_string(),
    }
}

fn save_registry(path: &Path, entries: &[(&str, &Path)]) {
    let mut store = registry::RegistryStore::default();
    for (slug, repo) in entries {
        store.upsert(slug, &repo.to_string_lossy());
    }
    registry::save_to(&store, path).unwrap();
}

async fn serve(identity: Identity, registry_path: PathBuf, auth: Arc<AuthState>) -> u16 {
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
    tokio::spawn(async move {
        let _shutdown = tx;
        axum::serve(listener, app).await.unwrap();
    });
    port
}

fn descriptor(id: &str, port: u16) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: id.to_string(),
        name: "peer".to_string(),
        avatar: "🐙".to_string(),
        address: "127.0.0.1".to_string(),
        port,
        environment: "WSL: Ubuntu".to_string(),
        os: String::new(),
        token: "peer-tok".to_string(),
        protocol_version: PEER_PROTOCOL_VERSION,
        tunnel: None,
        nudge: None,
    }
}

/// Every reply frame to one command, until the server closes the socket.
async fn ask(port: u16, verb: &str, payload: serde_json::Value) -> Vec<serde_json::Value> {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws/command"))
        .await
        .unwrap();
    ws.send(Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id: 1,
            verb: verb.to_string(),
            payload,
        }))
        .into(),
    ))
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut frames = Vec::new();
        while let Some(Ok(message)) = ws.next().await {
            if let Message::Binary(bytes) = message {
                if let Ok(Frame::Command(reply)) = protocol::decode(&bytes) {
                    frames.push(reply.payload);
                }
            }
        }
        frames
    })
    .await
    .expect("the server closes the socket")
}

/// The one reply of a one-reply verb.
async fn ask_one(port: u16, verb: &str, payload: serde_json::Value) -> serde_json::Value {
    let frames = ask(port, verb, payload).await;
    assert_eq!(frames.len(), 1, "one reply frame: {frames:?}");
    frames.into_iter().next().unwrap()
}

fn names(reply: &serde_json::Value) -> Vec<String> {
    reply["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("entries in {reply}"))
        .iter()
        .filter_map(|e| e["name"].as_str().map(str::to_string))
        .collect()
}

fn inside(dir: &Path) -> String {
    format!("{}{MAIN_SEPARATOR}", dir.display())
}

fn argv_lines(file: &Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

// One test: the exe override and the child's env are process-global.
#[tokio::test]
async fn registry_verbs_route_by_daemon_and_refuse_before_spawning() {
    let argv_dir = tempfile::tempdir().unwrap();
    let argv_file = argv_dir.path().join("argv.log");
    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_ARGV_FILE", &argv_file);

    // The peer: its own store, its own registry, one registered folder.
    let peer_disk = tempfile::tempdir().unwrap();
    let peer_repo = peer_disk.path().join("peer-repo");
    std::fs::create_dir_all(peer_repo.join(".git")).unwrap();
    let peer_store = tempfile::tempdir().unwrap();
    let peer_registry = peer_store.path().join("repos.toml");
    save_registry(&peer_registry, &[("o/peer", &peer_repo)]);
    let peer_port = serve(
        identity(B_ID),
        peer_registry,
        AuthState::fixed(
            AuthPolicy::Bearer("peer-tok".to_string()),
            SessionEpoch::in_memory_detached(),
        ),
    )
    .await;

    // The local daemon: an empty registry and a descriptor of the peer.
    let local_disk = tempfile::tempdir().unwrap();
    let local_repo = local_disk.path().join("alpha");
    std::fs::create_dir_all(&local_repo).unwrap();
    let local_store = tempfile::tempdir().unwrap();
    let local_registry = local_store.path().join("repos.toml");
    save_registry(&local_registry, &[]);
    peer::write_descriptor(local_store.path(), &descriptor(B_ID, peer_port)).unwrap();
    let port = serve(
        identity(A_ID),
        local_registry.clone(),
        AuthState::localhost(),
    )
    .await;

    // (a) `dir.list` with no repo and no daemon: this daemon's disk.
    let reply = ask_one(
        port,
        "dir.list",
        serde_json::json!({ "path": inside(local_disk.path()) }),
    )
    .await;
    assert_eq!(reply["status"], "ok", "{reply}");
    assert_eq!(names(&reply), ["alpha"]);
    // Its own id is the same daemon.
    let reply = ask_one(
        port,
        "dir.list",
        serde_json::json!({ "daemon": A_ID, "path": inside(local_disk.path()) }),
    )
    .await;
    assert_eq!(names(&reply), ["alpha"], "{reply}");

    // (b) With the peer's id, the PEER answers: its registry marks the folder
    // added, which the local (empty) registry never would.
    let reply = ask_one(
        port,
        "dir.list",
        serde_json::json!({ "daemon": B_ID, "path": inside(peer_disk.path()) }),
    )
    .await;
    assert_eq!(reply["status"], "ok", "{reply}");
    assert_eq!(reply["entries"][0]["name"], "peer-repo", "{reply}");
    assert_eq!(reply["entries"][0]["added"], true, "{reply}");
    assert_eq!(reply["entries"][0]["repo"], true, "{reply}");
    golden::check("dir.list", reply, &["/dir/path"]);

    // (c) An unknown id is an error, for both verbs.
    for verb in ["dir.list", "project.add"] {
        let reply = ask_one(
            port,
            verb,
            serde_json::json!({ "daemon": UNKNOWN_ID, "path": inside(local_disk.path()) }),
        )
        .await;
        assert_eq!(reply["status"], "error", "{verb}: {reply}");
        assert!(
            reply["message"].as_str().unwrap().contains(UNKNOWN_ID),
            "{verb}: {reply}"
        );
    }

    // (d) A bad path: one error frame, and nothing spawns.
    let control = format!("{}\u{7}", local_repo.display());
    for path in [
        serde_json::json!(""),
        serde_json::json!("relative/dir"),
        serde_json::json!(format!("{}{}", local_repo.display(), "a".repeat(4096))),
        serde_json::json!(control),
    ] {
        let reply = ask_one(port, "project.add", serde_json::json!({ "path": path })).await;
        assert_eq!(reply["status"], "error", "{path}: {reply}");
    }
    assert!(argv_lines(&argv_file).is_empty(), "nothing spawned");

    // (e) A valid path spawns `daemon add`, `--init` only when asked and `--`
    // before the path. The child does not write the registry, so the entry
    // the real CLI would write is seeded first.
    save_registry(&local_registry, &[("o/alpha", &local_repo)]);
    let path = local_repo.to_string_lossy().into_owned();
    let reply = ask_one(
        port,
        "project.add",
        serde_json::json!({ "path": path, "init": true }),
    )
    .await;
    assert_eq!(reply["status"], "ok", "{reply}");
    assert_eq!(reply["slug"], "o/alpha", "{reply}");
    golden::check("project.add", reply, &["/path"]);
    let reply = ask_one(port, "project.add", serde_json::json!({ "path": path })).await;
    assert_eq!(reply["status"], "ok", "{reply}");
    assert_eq!(
        argv_lines(&argv_file),
        [
            format!("daemon add --init -- {path}"),
            format!("daemon add -- {path}"),
        ]
    );

    // (f) A refusal of the CLI is an error that carries its text.
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "1");
    let reply = ask_one(port, "project.add", serde_json::json!({ "path": path })).await;
    std::env::remove_var("RALPHY_TEST_EXIT_CODE");
    assert_eq!(reply["status"], "error", "{reply}");
    assert!(
        reply["message"]
            .as_str()
            .unwrap()
            .contains("command_test_child exiting 1"),
        "{reply}"
    );

    // (g) `project.add` with the peer's id runs on the peer, against the
    // peer's registry.
    let peer_path = peer_repo.to_string_lossy().into_owned();
    let reply = ask_one(
        port,
        "project.add",
        serde_json::json!({ "daemon": B_ID, "path": peer_path }),
    )
    .await;
    assert_eq!(reply["status"], "ok", "{reply}");
    assert_eq!(reply["slug"], "o/peer", "{reply}");
}
