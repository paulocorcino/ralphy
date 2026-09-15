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

/// `tree.find` (ADR-0036 amendment 2026-09-15) answers on the id without a
/// spawn, with `/`-joined rel paths, and with the TREE's policy: an ignored
/// dotfolder is found (the tree shows it), the noise dirs never are.
#[tokio::test]
async fn tree_find_answers_on_id_without_spawn() {
    let (url, slug) = serve_git_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        1,
        "tree.find",
        serde_json::json!({ "repo": slug, "query": "KEY" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "an Observe search must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    assert_eq!(reply["truncated"], false);
    let paths: Vec<&str> = reply["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|h| h["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec![".secret/key"], "paths={paths:?}");

    // NEGATIVE CONTROL: a name inside a noise dir is not found.
    let (replies, _) = round_trip(
        &url,
        2,
        "tree.find",
        serde_json::json!({ "repo": slug, "query": "junk" }),
    )
    .await;
    assert_eq!(replies[0]["hits"].as_array().unwrap().len(), 0);
}

/// `tree.grep` consults `.gitignore` — the one policy divergence from the
/// tree — but `.ralphy/` is always searched, and the count is per occurrence.
#[tokio::test]
async fn tree_grep_respects_gitignore_but_searches_ralphy() {
    let (url, slug) = serve_git_repo().await;
    // "plan" is in BOTH `.ralphy/plan.md` (ignored, always searched) and
    // `.secret/key` (ignored, honoured): only the former is a hit.
    let (replies, spawned) = round_trip(
        &url,
        1,
        "tree.grep",
        serde_json::json!({ "repo": slug, "query": "PLAN" }),
    )
    .await;

    assert_eq!(replies.len(), 1);
    assert_eq!(spawned, 0, "an Observe search must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    let hits: Vec<(&str, u64)> = reply["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|h| (h["path"].as_str().unwrap(), h["count"].as_u64().unwrap()))
        .collect();
    assert_eq!(hits, vec![(".ralphy/plan.md", 1)], "hits={hits:?}");

    // NEGATIVE CONTROL: `tree.find` on the same repo DOES see the ignored
    // file — the two policies differ on purpose.
    let (replies, _) = round_trip(
        &url,
        2,
        "tree.find",
        serde_json::json!({ "repo": slug, "query": "key" }),
    )
    .await;
    assert_eq!(replies[0]["hits"][0]["path"], ".secret/key");
}

/// An unknown repo is refused the same way every Observe verb is.
#[tokio::test]
async fn tree_find_refuses_an_unknown_repo() {
    let (url, _slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        1,
        "tree.find",
        serde_json::json!({ "repo": "owner/nowhere", "query": "vis" }),
    )
    .await;
    assert_eq!(spawned, 0);
    assert_eq!(replies[0]["status"], "error");
}

/// Bind a daemon over a temp repo that also holds a linked worktree `wt-a`
/// under `.ralphy/worktrees/` — the `.git` POINTER file git writes for one,
/// with its own files (`inner.txt`, `sub/deep.txt`) that the primary lacks.
/// Returns the `ws://…/ws/command` URL, the slug, and the primary's path (so a
/// test can prove a refused write never landed there).
async fn serve_checkout_repo() -> (String, String, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("visible.txt"), b"hello").unwrap();
    std::fs::write(dir.path().join("only.txt"), b"primary").unwrap();
    let wt = dir.path().join(".ralphy/worktrees/wt-a");
    std::fs::create_dir_all(wt.join("sub")).unwrap();
    let gitdir = dir
        .path()
        .join(".git/worktrees/wt-a")
        .to_string_lossy()
        .replace('\\', "/");
    std::fs::write(wt.join(".git"), format!("gitdir: {gitdir}\n")).unwrap();
    std::fs::write(wt.join("inner.txt"), b"inside").unwrap();
    std::fs::write(wt.join("sub/deep.txt"), b"x").unwrap();

    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/checkout";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();
    let root = dir.path().to_path_buf();
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
        root,
    )
}

fn entry_names(reply: &serde_json::Value) -> Vec<&str> {
    reply["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect()
}

/// `checkout` (ADR-0036 amendment, ADR-0063 §2) prefixes the rel under the
/// SAME root: `tree.list` at `""` lists the WORKTREE's root, not the primary's.
#[tokio::test]
async fn tree_list_with_checkout_lists_the_worktree() {
    let (url, slug, _root) = serve_checkout_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        1,
        "tree.list",
        serde_json::json!({ "repo": slug, "path": "", "checkout": "wt-a" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "an Observe read must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    let names = entry_names(reply);
    assert!(names.contains(&"inner.txt"), "names={names:?}");
    assert!(names.contains(&"sub"), "names={names:?}");
    assert!(
        !names.contains(&"visible.txt"),
        "the primary's file must not leak into the worktree listing: {names:?}"
    );
    assert!(
        !names.contains(&".git"),
        "the pointer file is noise: {names:?}"
    );
}

#[tokio::test]
async fn tree_list_with_checkout_and_subdir_prefixes_the_rel() {
    let (url, slug, _root) = serve_checkout_repo().await;
    let (replies, _) = round_trip(
        &url,
        2,
        "tree.list",
        serde_json::json!({ "repo": slug, "path": "sub", "checkout": "wt-a" }),
    )
    .await;
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    assert_eq!(entry_names(reply), vec!["deep.txt"]);
}

#[tokio::test]
async fn file_read_with_checkout_reads_the_worktree_file() {
    let (url, slug, _root) = serve_checkout_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        3,
        "file.read",
        serde_json::json!({ "repo": slug, "path": "inner.txt", "checkout": "wt-a" }),
    )
    .await;
    assert_eq!(spawned, 0);
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    assert_eq!(reply["content"], "inside");

    // NEGATIVE CONTROL: the same rel WITHOUT `checkout` is the primary's miss.
    let (replies, _) = round_trip(
        &url,
        4,
        "file.read",
        serde_json::json!({ "repo": slug, "path": "inner.txt" }),
    )
    .await;
    assert_eq!(replies[0]["status"], "error", "reply={}", replies[0]);
}

/// A hit inside the worktree comes back relative to the WORKTREE (the
/// operator's rel), never with the `.ralphy/worktrees/wt-a/` prefix.
#[tokio::test]
async fn tree_find_with_checkout_answers_unprefixed_paths() {
    let (url, slug, _root) = serve_checkout_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        5,
        "tree.find",
        serde_json::json!({ "repo": slug, "query": "inner", "checkout": "wt-a" }),
    )
    .await;
    assert_eq!(spawned, 0);
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    assert_eq!(
        reply["hits"],
        serde_json::json!([{ "path": "inner.txt", "dir": false }])
    );

    // The same query WITHOUT `checkout` finds it under the primary's rel —
    // the prefix is what the checkout strips, not the walk.
    let (replies, _) = round_trip(
        &url,
        6,
        "tree.find",
        serde_json::json!({ "repo": slug, "query": "inner" }),
    )
    .await;
    assert_eq!(
        replies[0]["hits"][0]["path"],
        ".ralphy/worktrees/wt-a/inner.txt"
    );
}

/// A name with no pointer file, or one that fails the shape gate, is refused
/// by NAME with the one message the shell resets its selection on.
#[tokio::test]
async fn unknown_checkout_is_refused_by_name() {
    let (url, slug, root) = serve_checkout_repo().await;
    // Plant a pointer file where `../x` WOULD resolve if the shape gate were
    // skipped and the name joined first (`.ralphy/worktrees/../x/.git`): a
    // gate-less daemon would then prefix, fail `confine` on the `..`, and
    // answer `not found` — not `unknown checkout`. The reply discriminates.
    std::fs::create_dir_all(root.join(".ralphy/x")).unwrap();
    std::fs::write(root.join(".ralphy/x/.git"), "gitdir: /r/.git/worktrees/x\n").unwrap();
    let unknown = serde_json::json!({ "status": "error", "message": "unknown checkout" });
    for (id, name) in [(7u64, "nope"), (8, "../x"), (9, ""), (12, "C:")] {
        let (replies, spawned) = round_trip(
            &url,
            id,
            "tree.list",
            serde_json::json!({ "repo": slug, "path": "", "checkout": name }),
        )
        .await;
        assert_eq!(spawned, 0);
        assert_eq!(replies.len(), 1, "one reply for {name:?}");
        assert_eq!(replies[0], unknown, "checkout {name:?}");
    }
    // `null` is the primary tree, not an unknown name.
    let (replies, _) = round_trip(
        &url,
        10,
        "tree.list",
        serde_json::json!({ "repo": slug, "path": "", "checkout": null }),
    )
    .await;
    assert_eq!(replies[0]["status"], "ok", "reply={}", replies[0]);
    assert!(entry_names(&replies[0]).contains(&"visible.txt"));
}

/// A Write verb carrying `checkout` is refused (the `.ralphy` denylist would
/// refuse the prefixed path anyway) — and the PRIMARY's file at the same rel
/// is the negative control: the write must not land there either.
#[tokio::test]
async fn file_write_with_checkout_is_refused() {
    let (url, slug, root) = serve_checkout_repo().await;
    let (replies, spawned) = round_trip(
        &url,
        11,
        "file.write",
        serde_json::json!({
            "repo": slug,
            "path": "visible.txt",
            "content": "clobbered",
            "checkout": "wt-a",
        }),
    )
    .await;
    assert_eq!(spawned, 0);
    let reply = &replies[0];
    assert_eq!(reply["status"], "error", "reply={reply}");
    assert_eq!(reply["reason"], "refused", "reply={reply}");
    assert_eq!(
        reply["message"],
        "writes inside a worktree are not available yet"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("visible.txt")).unwrap(),
        "hello",
        "the primary's file must be untouched"
    );
    assert!(
        !root.join(".ralphy/worktrees/wt-a/visible.txt").exists(),
        "nothing lands in the worktree either"
    );
}

/// A worktree dir that is a SYMLINK out of the repo — its target carrying a
/// real `gitdir:` pointer, as any linked worktree elsewhere on the host does —
/// must be refused by the searches exactly as `tree.list` refuses it: the walk
/// root is resolved through `confine` against the REGISTERED root, never
/// handed to the walker as a root that would confine against itself.
#[tokio::test]
async fn search_with_a_symlinked_checkout_is_refused_like_a_listing() {
    let (url, slug, root) = serve_checkout_repo().await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(
        outside.path().join(".git"),
        "gitdir: /elsewhere/.git/worktrees/wt-out\n",
    )
    .unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"leak").unwrap();
    let link = root.join(".ralphy/worktrees/wt-out");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(outside.path(), &link);
    #[cfg(not(windows))]
    let linked = std::os::unix::fs::symlink(outside.path(), &link);
    if let Err(e) = linked {
        // A Windows host without the symlink privilege cannot stage the escape;
        // the Linux leg of CI does. Say so rather than pass vacuously.
        eprintln!("SKIPPED: cannot create a directory symlink here: {e}");
        return;
    }
    std::mem::forget(outside);

    for (id, verb) in [(20u64, "tree.find"), (21, "tree.grep"), (22, "tree.list")] {
        let payload = if verb == "tree.list" {
            serde_json::json!({ "repo": slug, "path": "", "checkout": "wt-out" })
        } else {
            serde_json::json!({ "repo": slug, "query": "secret", "checkout": "wt-out" })
        };
        let (replies, spawned) = round_trip(&url, id, verb, payload).await;
        assert_eq!(spawned, 0);
        let reply = &replies[0];
        assert_eq!(reply["status"], "error", "{verb} must refuse: {reply}");
        assert!(
            reply["hits"].is_null() && reply["entries"].is_null(),
            "{verb} leaked through the link: {reply}"
        );
    }
}
