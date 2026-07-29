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
        token: "peer-tok".to_string(),
        protocol_version: PEER_PROTOCOL_VERSION,
        nudge: None,
    }
}

async fn ask(port: u16, id: u64, verb: &str, payload: serde_json::Value) -> serde_json::Value {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws/command"))
        .await
        .unwrap();
    ws.send(Message::Binary(protocol::encode(&Frame::Command(
        Command {
            id,
            verb: verb.to_string(),
            payload,
        },
    ))))
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
    ws.send(Message::Binary(protocol::encode(&Frame::Command(
        Command {
            id,
            verb: verb.to_string(),
            payload,
        },
    ))))
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

#[tokio::test]
async fn peer_spawn_verbs_stream_from_the_owning_daemon() {
    let peer_store = tempfile::tempdir().unwrap();
    let peer_repo = tempfile::tempdir().unwrap();
    std::fs::write(peer_repo.path().join("note.txt"), "peer-side").unwrap();
    let png = b"\x89PNG\r\n\x1a\npeer-fixture";
    std::fs::write(peer_repo.path().join("pixel.png"), png).unwrap();
    let mut large_png = vec![0_u8; ralphy_daemon::tree::MAX_IMAGE_BYTES as usize];
    large_png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    std::fs::write(peer_repo.path().join("large.png"), &large_png).unwrap();
    let runid = "01TESTRUNIDTESTRUNIDTE";
    let runstate = peer_repo.path().join(".ralphy").join("runstate");
    std::fs::create_dir_all(&runstate).unwrap();
    std::fs::write(
        runstate.join(format!("{runid}.json")),
        serde_json::to_vec(&serde_json::json!({
            "v": 1,
            "runid": runid,
            "pid": std::process::id(),
            "started_at": "2026-07-29T10:00:00-03:00"
        }))
        .unwrap(),
    )
    .unwrap();
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
    std::fs::write(local_repo.path().join("note.txt"), "local-side").unwrap();
    std::fs::write(local_repo.path().join("local-only.txt"), "local").unwrap();
    let local_registry = local_store.path().join("repos.toml");
    save_registry(&local_registry, local_repo.path());
    peer::write_descriptor(local_store.path(), &descriptor(B_ID, peer_port)).unwrap();
    let closed_port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    peer::write_descriptor(local_store.path(), &descriptor(DEAD_ID, closed_port)).unwrap();
    let (local_port, _local_task) = serve(
        identity(A_ID, "local"),
        local_registry,
        AuthState::localhost(),
    )
    .await;

    let read = ask(
        local_port,
        1,
        "file.read",
        serde_json::json!({
            "repo": format!("{B_ID}/{SLUG}"),
            "path": "note.txt"
        }),
    )
    .await;
    assert_eq!(read["content"], "peer-side", "got {read}");
    assert_ne!(
        read["content"], "local-side",
        "the local registry must never answer for a peer ref"
    );

    let tree = ask(
        local_port,
        4,
        "tree.list",
        serde_json::json!({ "repo": format!("{B_ID}/{SLUG}"), "path": "" }),
    )
    .await;
    let names: Vec<&str> = tree["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(names.contains(&"note.txt"), "got {tree}");
    assert!(
        !names.contains(&"local-only.txt"),
        "a peer tree must not come from the local collision: {tree}"
    );

    let image = ask(
        local_port,
        5,
        "file.image",
        serde_json::json!({
            "repo": format!("{B_ID}/{SLUG}"),
            "path": "pixel.png"
        }),
    )
    .await;
    assert_eq!(image["mediaType"], "image/png", "got {image}");
    assert_eq!(
        data_encoding::BASE64
            .decode(image["base64"].as_str().unwrap().as_bytes())
            .unwrap(),
        png
    );
    let large_image = ask(
        local_port,
        14,
        "file.image",
        serde_json::json!({
            "repo": format!("{B_ID}/{SLUG}"),
            "path": "large.png"
        }),
    )
    .await;
    let decoded_large = data_encoding::BASE64
        .decode(large_image["base64"].as_str().unwrap().as_bytes())
        .unwrap();
    assert_eq!(decoded_large.len(), large_png.len());
    assert_eq!(&decoded_large[..8], b"\x89PNG\r\n\x1a\n");

    let runs = ask(
        local_port,
        6,
        "runs.list",
        serde_json::json!({ "repo": format!("{B_ID}/{SLUG}") }),
    )
    .await;
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1, "got {runs}");
    assert_eq!(runs["runs"][0]["runid"], runid, "got {runs}");

    let peer_ref = format!("{B_ID}/{SLUG}");
    let created = ask(
        local_port,
        7,
        "file.create",
        serde_json::json!({ "repo": peer_ref, "path": "new.txt", "dir": false }),
    )
    .await;
    assert_eq!(created["status"], "ok", "got {created}");
    assert!(peer_repo.path().join("new.txt").is_file());
    assert!(!local_repo.path().join("new.txt").exists());

    let written = ask(
        local_port,
        8,
        "file.write",
        serde_json::json!({
            "repo": peer_ref,
            "path": "new.txt",
            "content": "written-through-the-peer"
        }),
    )
    .await;
    assert_eq!(written["status"], "ok", "got {written}");
    assert_eq!(
        std::fs::read_to_string(peer_repo.path().join("new.txt")).unwrap(),
        "written-through-the-peer"
    );

    let renamed = ask(
        local_port,
        9,
        "file.rename",
        serde_json::json!({
            "repo": peer_ref,
            "path": "new.txt",
            "to": "renamed.txt"
        }),
    )
    .await;
    assert_eq!(renamed["status"], "ok", "got {renamed}");
    assert!(peer_repo.path().join("renamed.txt").is_file());
    assert!(!local_repo.path().join("renamed.txt").exists());

    let deleted = ask(
        local_port,
        10,
        "file.delete",
        serde_json::json!({ "repo": peer_ref, "path": "renamed.txt" }),
    )
    .await;
    assert_eq!(deleted["status"], "ok", "got {deleted}");
    assert!(!peer_repo.path().join("renamed.txt").exists());

    let escape_name = format!("escape-{B_ID}.txt");
    let escaped = ask(
        local_port,
        11,
        "file.write",
        serde_json::json!({
            "repo": peer_ref,
            "path": format!("../{escape_name}"),
            "content": "escaped"
        }),
    )
    .await;
    assert_eq!(escaped["status"], "error", "got {escaped}");
    assert_eq!(escaped["reason"], "refused", "got {escaped}");
    assert!(!peer_repo
        .path()
        .parent()
        .unwrap()
        .join(&escape_name)
        .exists());
    assert!(!local_repo
        .path()
        .parent()
        .unwrap()
        .join(&escape_name)
        .exists());

    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "1");
    for (id, verb, payload, argv) in [
        (
            20,
            "changes.list",
            serde_json::json!({ "repo": peer_ref }),
            "changes list --format json",
        ),
        (
            21,
            "changes.stage",
            serde_json::json!({ "repo": peer_ref, "paths": ["a.txt"] }),
            "changes stage --path=a.txt",
        ),
        (
            22,
            "changes.unstage",
            serde_json::json!({ "repo": peer_ref, "paths": ["a.txt"] }),
            "changes unstage --path=a.txt",
        ),
        (
            23,
            "changes.discard",
            serde_json::json!({ "repo": peer_ref, "paths": ["a.txt"] }),
            "changes discard --path=a.txt",
        ),
        (
            24,
            "changes.commit",
            serde_json::json!({ "repo": peer_ref, "message": "peer commit" }),
            "changes commit --message=peer commit",
        ),
    ] {
        let reply = ask(local_port, id, verb, payload).await;
        let message = reply["message"]
            .as_str()
            .unwrap_or_else(|| panic!("{verb} did not relay child output: {reply}"));
        assert!(
            message.contains(&format!("dispatch-cwd: {}", peer_repo.path().display())),
            "{verb} must execute in the peer repo: {message}"
        );
        assert!(
            !message.contains(&local_repo.path().display().to_string()),
            "{verb} must never execute in the local collision: {message}"
        );
        assert!(
            message.contains(argv),
            "{verb} must carry its own argv: {message}"
        );
    }
    let config_refused = ask(
        local_port,
        25,
        "config.set",
        serde_json::json!({
            "repo": peer_ref,
            "key": "verify.command",
            "value": "echo widened"
        }),
    )
    .await;
    assert_eq!(
        config_refused["message"], "invalid mutation options",
        "the owning daemon must retain the remote config boundary: {config_refused}"
    );

    std::env::set_var("RALPHY_TEST_EXIT_CODE", "0");
    let env_dump = peer_store.path().join("peer-command-env.txt");
    std::env::set_var("RALPHY_TEST_ENV_DUMP", &env_dump);
    for (id, verb, payload) in [
        (
            12,
            "run",
            serde_json::json!({
                "repo": format!("{B_ID}/{SLUG}"),
                "agent": "codex",
                "branchMode": "current"
            }),
        ),
        (
            13,
            "triage",
            serde_json::json!({ "repo": format!("{B_ID}/{SLUG}") }),
        ),
        (
            14,
            "push",
            serde_json::json!({ "repo": format!("{B_ID}/{SLUG}") }),
        ),
    ] {
        let replies = ask_all(local_port, id, verb, payload).await;
        let statuses = replies
            .iter()
            .filter_map(|reply| reply.payload["status"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(statuses.first(), Some(&"spawned"), "{verb}: {statuses:?}");
        assert_eq!(statuses.last(), Some(&"exited"), "{verb}: {statuses:?}");
        assert!(
            statuses[1..statuses.len() - 1]
                .iter()
                .all(|status| *status == "output"),
            "{verb}: {statuses:?}"
        );
        let output = replies
            .iter()
            .filter_map(|reply| reply.payload["chunk"].as_str())
            .collect::<String>();
        assert!(
            output.contains("dispatch-stdout-marker"),
            "{verb}: {output}"
        );
        assert!(
            output.contains("dispatch-stderr-marker"),
            "{verb}: {output}"
        );
        assert!(
            output.contains(&format!("dispatch-cwd: {}", peer_repo.path().display())),
            "{verb} must execute in the peer repo: {output}"
        );
        assert!(
            !output.contains(&local_repo.path().display().to_string()),
            "{verb} must never execute in the local collision: {output}"
        );
        assert_eq!(
            replies.last().unwrap().payload["code"],
            0,
            "{verb}: {replies:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&env_dump).unwrap(),
            format!("RALPHY_DAEMON_TOKEN=ABSENT\nRALPHY_DAEMON_ID={B_ID}"),
            "{verb} must carry the owning daemon identity"
        );
    }
    std::env::remove_var("RALPHY_TEST_ENV_DUMP");

    let unreachable = ask(
        local_port,
        15,
        "file.read",
        serde_json::json!({
            "repo": format!("{DEAD_ID}/{SLUG}"),
            "path": "note.txt"
        }),
    )
    .await;
    let message = unreachable["message"].as_str().unwrap();
    assert!(message.contains(ENV), "got {unreachable}");
    assert!(
        !message.contains("serde") && !message.contains("decod"),
        "transport failure must not surface as a decoding error: {unreachable}"
    );
}
