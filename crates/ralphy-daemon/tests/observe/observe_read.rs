//! The Observe read path answers on the requesting `Command` id and NEVER
//! spawns (issue #194; ADR-0036 §2/§4/§5): a `tree.list` returns the confined,
//! gitignore-filtered directory listing and a `file.read` of a binary file is
//! refused with a reason — both on the SAME `id`, with ZERO `status:"spawned"`
//! frames (no per-read `ralphy` process). Mirrors `tests/command_refusal.rs`.

use std::time::{Duration, Instant};

use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

use crate::page;

mod checkout;
mod file_reads;
mod tree_reads;

/// The seeded `logo.png`'s bytes: a PNG signature plus a little payload, so the
/// reply's base64 has something to round-trip.
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";

/// Bind a daemon over a temp repo seeded with `visible.txt`, `node_modules/junk`,
/// a binary `bin.dat`, a real `logo.png` and an HTML-in-`.png` `evil.png`; return
/// the daemon's port and the repo slug.
///
/// The repo is `outer/repo`, and a real text `outer/secret` and a real image
/// `outer/secret.png` sit right outside it. So a `../secret` read can answer
/// "not found" only because confinement refused it, never because the target
/// is absent.
pub(super) async fn serve_repo() -> (u16, String) {
    let outer = tempfile::tempdir().expect("create the temp dir");
    std::fs::write(outer.path().join("secret"), b"token").expect("write secret");
    std::fs::write(outer.path().join("secret.png"), PNG_BYTES).expect("write secret.png");
    let dir = outer.path().join("repo");
    std::fs::create_dir(&dir).expect("create the test directory");
    std::fs::write(dir.join("visible.txt"), b"hello").expect("write visible.txt");
    std::fs::create_dir(dir.join("node_modules")).expect("create the directory node_modules");
    std::fs::write(dir.join("node_modules/junk"), b"x").expect("write node_modules/junk");
    std::fs::write(dir.join("bin.dat"), [0x00, 0x01, 0x02]).expect("write bin.dat");
    std::fs::write(dir.join("logo.png"), PNG_BYTES).expect("write logo.png");
    std::fs::write(dir.join("evil.png"), b"<html><script>x</script>").expect("write evil.png");

    let registry_path = dir.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/observe";
    store.upsert(slug, &dir.to_string_lossy());
    registry::save_to(&store, &registry_path).expect("save the registry");
    // Leak the tempdir so the registered repo outlives this fn (the daemon reads
    // it on every command); the OS reclaims it when the test process exits.
    std::mem::forget(outer);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
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
        axum::serve(listener, app)
            .await
            .expect("serve the test app");
    });
    (port, slug.to_string())
}

/// Bind a daemon over a temp *git* repo seeded with committed dot-folders
/// (`.github`, `.ralphy/plan.md`), noise dirs (`node_modules`, `target`), a
/// gitignored `.secret/`, and a `visible.txt`. `git init` stays even though the
/// listing no longer consults `.gitignore`: it is what makes `.secret/` a
/// genuinely ignored entry, so the test proves the amendment rather than a
/// no-op. Returns the daemon's port and the repo slug.
pub(super) async fn serve_git_repo() -> (u16, String) {
    let dir = tempfile::tempdir().expect("create the temp dir");
    std::fs::write(dir.path().join("visible.txt"), b"hello").expect("write visible.txt");
    std::fs::create_dir(dir.path().join(".github")).expect("create the directory .github");
    std::fs::write(dir.path().join(".github/config.yml"), b"x").expect("write .github/config.yml");
    std::fs::create_dir(dir.path().join(".ralphy")).expect("create the directory .ralphy");
    std::fs::write(dir.path().join(".ralphy/plan.md"), b"# plan").expect("write .ralphy/plan.md");
    std::fs::create_dir(dir.path().join("node_modules"))
        .expect("create the directory node_modules");
    std::fs::write(dir.path().join("node_modules/junk"), b"x").expect("write node_modules/junk");
    std::fs::create_dir(dir.path().join("target")).expect("create the directory target");
    std::fs::write(dir.path().join("target/out"), b"x").expect("write target/out");
    std::fs::create_dir(dir.path().join(".secret")).expect("create the directory .secret");
    std::fs::write(dir.path().join(".secret/key"), b"x").expect("write .secret/key");
    std::fs::write(dir.path().join(".gitignore"), b".secret/\n").expect("write .gitignore");

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
    registry::save_to(&store, &registry_path).expect("save the registry");
    std::mem::forget(dir);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
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
        axum::serve(listener, app)
            .await
            .expect("serve the test app");
    });
    (port, slug.to_string())
}

/// Send one `Command` on a `spawn` channel and collect every reply frame on
/// `id` until the channel ends, returning `(replies, spawned_count)`.
pub(super) async fn round_trip(
    port: u16,
    id: u64,
    verb: &str,
    payload: serde_json::Value,
) -> (Vec<serde_json::Value>, usize) {
    let mut ws = page::command(port).await;
    ws.send(Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id,
            verb: verb.to_string(),
            payload,
        }))
        .into(),
    ))
    .await
    .expect("send the frame");

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
    .expect("the reply must arrive and the channel close within 10s")
}

/// Bind a daemon over a temp repo that also holds a linked worktree `wt-a`
/// under `.ralphy/worktrees/` — the `.git` POINTER file git writes for one,
/// with its own files (`inner.txt`, `sub/deep.txt`) that the primary lacks.
/// Returns the daemon's port, the slug, and the primary's path (so a
/// test can prove a refused write never landed there).
pub(super) async fn serve_checkout_repo() -> (u16, String, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("create the temp dir");
    std::fs::write(dir.path().join("visible.txt"), b"hello").expect("write visible.txt");
    std::fs::write(dir.path().join("only.txt"), b"primary").expect("write only.txt");
    let wt = dir.path().join(".ralphy/worktrees/wt-a");
    std::fs::create_dir_all(wt.join("sub")).expect("create the directory sub");
    let gitdir = dir
        .path()
        .join(".git/worktrees/wt-a")
        .to_string_lossy()
        .replace('\\', "/");
    std::fs::write(wt.join(".git"), format!("gitdir: {gitdir}\n")).expect("write .git");
    std::fs::write(wt.join("inner.txt"), b"inside").expect("write inner.txt");
    std::fs::write(wt.join("sub/deep.txt"), b"x").expect("write sub/deep.txt");
    // The primary ignores `.ralphy/`, as every repo ralphy touches does; the
    // worktree has its own rules.
    std::fs::write(dir.path().join(".gitignore"), b".ralphy/\n").expect("write .gitignore");
    std::fs::write(wt.join(".gitignore"), b"*.log\n").expect("write .gitignore");
    std::fs::write(wt.join("build.log"), b"x").expect("write build.log");

    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/checkout";
    store.upsert(slug, &dir.path().to_string_lossy());
    registry::save_to(&store, &registry_path).expect("save the registry");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
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
        axum::serve(listener, app)
            .await
            .expect("serve the test app");
    });
    (port, slug.to_string(), root)
}

fn entry_names(reply: &serde_json::Value) -> Vec<&str> {
    reply["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|e| e["name"].as_str().expect("read the string field"))
        .collect()
}
