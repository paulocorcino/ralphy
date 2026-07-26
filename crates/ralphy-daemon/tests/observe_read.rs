//! The Observe read path answers on the requesting `Command` id and NEVER
//! spawns (issue #194; ADR-0036 §2/§4/§5): a `tree.list` returns the confined,
//! gitignore-filtered directory listing and a `file.read` of a binary file is
//! refused with a reason — both on the SAME `id`, with ZERO `status:"spawned"`
//! frames (no per-read `ralphy` process). Mirrors `tests/command_refusal.rs`.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

/// The seeded `logo.png`'s bytes: a PNG signature plus a little payload, so the
/// reply's base64 has something to round-trip.
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";

/// Bind a daemon over a temp repo seeded with `visible.txt`, `node_modules/junk`,
/// a binary `bin.dat`, a real `logo.png` and an HTML-in-`.png` `evil.png`; return
/// the `ws://…/ws/command` URL and the repo slug.
async fn serve_repo() -> (String, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("visible.txt"), b"hello").unwrap();
    std::fs::create_dir(dir.path().join("node_modules")).unwrap();
    std::fs::write(dir.path().join("node_modules/junk"), b"x").unwrap();
    std::fs::write(dir.path().join("bin.dat"), [0x00, 0x01, 0x02]).unwrap();
    std::fs::write(dir.path().join("logo.png"), PNG_BYTES).unwrap();
    std::fs::write(dir.path().join("evil.png"), b"<html><script>x</script>").unwrap();

    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/observe";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    // Leak the tempdir so the registered repo outlives this fn (the daemon reads
    // it on every command); the OS reclaims it when the test process exits.
    std::mem::forget(dir);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    // Leak the shutdown sender so the channel stays open for the server's lifetime.
    std::mem::forget(_tx);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        format!("ws://127.0.0.1:{port}/ws/command"),
        slug.to_string(),
    )
}

/// Bind a daemon over a temp *git* repo seeded with committed dot-folders
/// (`.github`, `.ralphy/plan.md`), noise dirs (`node_modules`, `target`), a
/// gitignored `.secret/`, and a `visible.txt`. `git init` stays even though the
/// listing no longer consults `.gitignore`: it is what makes `.secret/` a
/// genuinely ignored entry, so the test proves the amendment rather than a
/// no-op. Returns the `ws://…/ws/command` URL and the repo slug.
async fn serve_git_repo() -> (String, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("visible.txt"), b"hello").unwrap();
    std::fs::create_dir(dir.path().join(".github")).unwrap();
    std::fs::write(dir.path().join(".github/config.yml"), b"x").unwrap();
    std::fs::create_dir(dir.path().join(".ralphy")).unwrap();
    std::fs::write(dir.path().join(".ralphy/plan.md"), b"# plan").unwrap();
    std::fs::create_dir(dir.path().join("node_modules")).unwrap();
    std::fs::write(dir.path().join("node_modules/junk"), b"x").unwrap();
    std::fs::create_dir(dir.path().join("target")).unwrap();
    std::fs::write(dir.path().join("target/out"), b"x").unwrap();
    std::fs::create_dir(dir.path().join(".secret")).unwrap();
    std::fs::write(dir.path().join(".secret/key"), b"x").unwrap();
    std::fs::write(dir.path().join(".gitignore"), b".secret/\n").unwrap();

    let status = std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .status()
        .expect("git must be installed to run this test (see environment.md)");
    assert!(status.success(), "git init failed in {:?}", dir.path());

    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/gitobserve";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    std::mem::forget(dir);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    std::mem::forget(_tx);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        format!("ws://127.0.0.1:{port}/ws/command"),
        slug.to_string(),
    )
}

/// Send one `Command` and collect every reply frame on `id` until the socket
/// closes, returning `(replies, spawned_count)`.
async fn round_trip(
    url: &str,
    id: u64,
    verb: &str,
    payload: serde_json::Value,
) -> (Vec<serde_json::Value>, usize) {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connecting to /ws/command");
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
        let mut spawned = 0usize;
        while let Some(msg) = ws.next().await {
            let bytes = match msg {
                Ok(Message::Binary(b)) => b,
                Ok(Message::Close(_)) | Err(_) => break,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.id != id {
                    continue;
                }
                if cmd.payload.get("status").and_then(|s| s.as_str()) == Some("spawned") {
                    spawned += 1;
                }
                replies.push(cmd.payload);
            }
        }
        (replies, spawned)
    })
    .await
    .expect("the reply must arrive and the socket close within 10s")
}

#[tokio::test]
async fn tree_list_answers_on_id_without_spawn() {
    let (url, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        1,
        "tree.list",
        serde_json::json!({ "repo": slug, "path": "" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "an Observe read must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok");
    let names: Vec<&str> = reply["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"visible.txt"), "names={names:?}");
    assert!(
        !names.contains(&"node_modules"),
        "noise filtered: {names:?}"
    );
}

#[tokio::test]
async fn tree_list_surfaces_committed_dotfolders_and_ralphy() {
    // A4 oracle (issue #203): `tree.list` at the repo root surfaces committed
    // dot-folders (`.github`), `.ralphy`, and — since ADR-0036's 2026-07-26
    // amendment — gitignored entries (`.secret`); only the `HARD_EXCLUDE` noise
    // dirs (`.git`, `node_modules`, `target`) are dropped.
    let (url, slug) = serve_git_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        4,
        "tree.list",
        serde_json::json!({ "repo": slug, "path": "" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "an Observe read must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok");
    let names: Vec<&str> = reply["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&".github"),
        "committed dot-folder: {names:?}"
    );
    assert!(names.contains(&".ralphy"), "`.ralphy` surfaced: {names:?}");
    assert!(!names.contains(&".git"), "`.git` dropped: {names:?}");
    assert!(
        !names.contains(&"node_modules"),
        "noise filtered: {names:?}"
    );
    assert!(!names.contains(&"target"), "noise filtered: {names:?}");
    // ADR-0036, amendment 2026-07-26: gitignored entries are LISTED. The operator
    // works in the ignored files, and `file.read` served them all along.
    assert!(names.contains(&".secret"), "gitignored listed: {names:?}");
}

#[tokio::test]
async fn file_read_refuses_binary() {
    let (url, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        2,
        "file.read",
        serde_json::json!({ "repo": slug, "path": "bin.dat" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "a refused read must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "error");
    let reason = reply["reason"].as_str().expect("a reason string");
    assert!(reason.contains("binary"), "reason={reason:?}");
}

#[tokio::test]
async fn image_read_serves_a_png_as_base64() {
    // ADR-0049 §2: one reply on the id, carrying the VERIFIED media type and the
    // bytes base64'd — and, like every Observe verb, zero spawns.
    let (url, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        5,
        "file.image",
        serde_json::json!({ "repo": slug, "path": "logo.png" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "an Observe read must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok");
    assert_eq!(reply["mediaType"], "image/png");
    let decoded = data_encoding::BASE64
        .decode(
            reply["base64"]
                .as_str()
                .expect("a base64 string")
                .as_bytes(),
        )
        .expect("the reply's base64 decodes");
    assert_eq!(decoded, PNG_BYTES, "the bytes survive the round trip");
}

#[tokio::test]
async fn image_read_refuses_bytes_that_belie_the_extension() {
    // The magic-byte check over the wire (ADR-0049 §3): HTML named `.png` is
    // refused, never handed to the browser labelled `image/png`.
    let (url, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        6,
        "file.image",
        serde_json::json!({ "repo": slug, "path": "evil.png" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "a refused read must never spawn");
    assert_eq!(replies[0]["status"], "error");
    assert_eq!(replies[0]["reason"], "not an image");
    assert!(
        replies[0].get("base64").is_none(),
        "a refusal carries no bytes: {:?}",
        replies[0]
    );
}

#[tokio::test]
async fn image_read_masks_traversal_as_not_found() {
    // Confinement is unchanged by ADR-0049: an out-of-root image read is a plain
    // miss, never leaking whether the target exists (ADR-0036 §5).
    let (url, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        7,
        "file.image",
        serde_json::json!({ "repo": slug, "path": "../secret.png" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "a refused read must never spawn");
    assert_eq!(replies[0]["status"], "error");
    assert_eq!(replies[0]["reason"], "not found");
}

#[tokio::test]
async fn file_read_masks_traversal_as_not_found() {
    // A `..` traversal over the wire must return a plain "not found", never
    // leaking whether the out-of-root target exists (ADR-0036 §5).
    let (url, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        3,
        "file.read",
        serde_json::json!({ "repo": slug, "path": "../secret" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "a refused read must never spawn");
    let reason = replies[0]["reason"].as_str().expect("a reason string");
    assert_eq!(replies[0]["status"], "error");
    assert!(reason.contains("not found"), "reason={reason:?}");
}
