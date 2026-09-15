//! The git-backed verbs take a selected `checkout` as the composed command's
//! `current_dir` (issue #407; ADR-0063 §2, ADR-0036 `checkout`). Each leg
//! spawns-and-collects `command_test_child` (via `RALPHY_EXE_OVERRIDE`), which
//! echoes its argv AND its cwd, so the hand-off is asserted on the wire: with
//! `checkout: "wt-a"` the cwd is `<root>/.ralphy/worktrees/wt-a` and the argv
//! is unchanged (no prefixed `--path`); without it the cwd is the registry
//! path; a verb outside the family keeps the registry path whatever the key
//! says; an unknown name answers `unknown checkout` and spawns NOTHING (the
//! `RALPHY_TEST_DONE_FILE` sentinel stays unwritten).
//!
//! The worktree is a pointer FILE (`.git` = `gitdir: …`) written by hand — the
//! resolver reads that file, never a git child (`checkout::is_linked`).
//!
//! SOLE env-setter in its file (see `command_config.rs`): `RALPHY_EXE_OVERRIDE`
//! and `RALPHY_TEST_*` are process-global. The legs run SEQUENTIALLY inside one
//! test so nothing races on them.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

/// One command round-trip on its own socket, returning the reply payload.
async fn ask(port: u16, id: u64, verb: &str, payload: serde_json::Value) -> serde_json::Value {
    let url = format!("ws://127.0.0.1:{port}/ws/command");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
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
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                Message::Close(_) => break,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.id == id {
                    return Some(cmd.payload);
                }
            }
        }
        None
    })
    .await
    .expect("a reply must arrive within 10s")
    .expect("a reply on the requesting id")
}

/// The child's echo out of a Query reply: exit 0 parses the stdout as JSON and
/// falls back to the raw text under the verb's field.
fn queried(reply: &serde_json::Value, field: &str) -> String {
    assert_eq!(reply["status"], "ok", "a zero exit answers ok; got {reply}");
    reply[field]
        .as_str()
        .unwrap_or_else(|| panic!("the child's echo under {field:?}; got {reply}"))
        .to_string()
}

/// The error message of a Mutate reply that relayed a non-zero exit.
fn relayed(reply: &serde_json::Value) -> String {
    assert_eq!(
        reply["status"], "error",
        "a non-zero exit relays as error; got {reply}"
    );
    reply["message"]
        .as_str()
        .unwrap_or_else(|| panic!("an error message string; got {reply}"))
        .to_string()
}

/// The `dispatch-cwd:` line of the child's echo.
fn cwd_of(text: &str) -> PathBuf {
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix("dispatch-cwd: "))
        .unwrap_or_else(|| panic!("a dispatch-cwd line in the echo; got {text:?}"));
    PathBuf::from(line.trim())
}

fn assert_in_worktree(text: &str, leg: &str) {
    let cwd = cwd_of(text);
    assert!(
        cwd.ends_with(".ralphy/worktrees/wt-a"),
        "{leg}: the child must run in the worktree; cwd was {cwd:?}"
    );
}

fn assert_at_root(text: &str, root: &Path, leg: &str) {
    let cwd = cwd_of(text);
    assert_eq!(
        cwd, root,
        "{leg}: the child must run at the registry path; cwd was {cwd:?}"
    );
}

#[tokio::test]
async fn git_backed_verbs_run_in_the_selected_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let registry_path = root.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    let slug = "owner/cwd";
    store.upsert(slug, &root.to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    // The linked worktree as git records it: a pointer FILE, no git spawn.
    let wt = root.join(".ralphy").join("worktrees").join("wt-a");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(wt.join(".git"), "gitdir: /r/.git/worktrees/wt-a\n").unwrap();

    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "0");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // (a) changes.list under the selection: worktree cwd, argv untouched.
    let text = queried(
        &ask(
            port,
            1,
            "changes.list",
            json!({ "repo": slug, "checkout": "wt-a" }),
        )
        .await,
        "changes",
    );
    assert_in_worktree(&text, "changes.list");
    assert!(
        text.contains("dispatch-argv: changes list --format json"),
        "changes.list argv unchanged under a checkout; got {text:?}"
    );

    // (b) blob.read: the path rides the argv UNPREFIXED — the cwd is the prefix.
    let text = queried(
        &ask(
            port,
            2,
            "blob.read",
            json!({ "repo": slug, "revision": "head", "path": "src/main.rs", "checkout": "wt-a" }),
        )
        .await,
        "blob",
    );
    assert_in_worktree(&text, "blob.read");
    assert!(
        text.contains("--path src/main.rs"),
        "blob.read path unprefixed; got {text:?}"
    );
    assert!(
        !text.contains("worktrees/wt-a/src"),
        "the worktree must not leak into the argv; got {text:?}"
    );

    // (c) branch.list runs in the worktree too: the picker's "current" is that tree's.
    let text = queried(
        &ask(
            port,
            3,
            "branch.list",
            json!({ "repo": slug, "checkout": "wt-a" }),
        )
        .await,
        "branches",
    );
    assert_in_worktree(&text, "branch.list");

    // (d) Outside the family the key is ignored: `.ralphy/` is the primary's.
    let text = queried(
        &ask(
            port,
            4,
            "config.get",
            json!({ "repo": slug, "key": "agent", "checkout": "wt-a" }),
        )
        .await,
        "config",
    );
    assert_at_root(&text, root, "config.get with checkout");
    let text = queried(
        &ask(
            port,
            5,
            "worktree.list",
            json!({ "repo": slug, "checkout": "wt-a" }),
        )
        .await,
        "checkouts",
    );
    assert_at_root(&text, root, "worktree.list with checkout");

    // (e) No checkout: the registry path, exactly as before.
    let text = queried(
        &ask(port, 6, "changes.list", json!({ "repo": slug })).await,
        "changes",
    );
    assert_at_root(&text, root, "changes.list without checkout");

    // (f) The Mutate arm: same cwd rule, argv unchanged, relayed on exit 1.
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "1");
    let msg = relayed(
        &ask(
            port,
            7,
            "changes.commit",
            json!({ "repo": slug, "message": "m", "checkout": "wt-a" }),
        )
        .await,
    );
    assert_in_worktree(&msg, "changes.commit");
    assert!(
        msg.contains("changes commit --message=m"),
        "commit argv unchanged; got {msg:?}"
    );
    let msg = relayed(
        &ask(
            port,
            8,
            "changes.stage",
            json!({ "repo": slug, "paths": ["a.txt"], "checkout": "wt-a" }),
        )
        .await,
    );
    assert_in_worktree(&msg, "changes.stage");
    assert!(
        msg.contains("changes stage --path=a.txt"),
        "stage argv unchanged; got {msg:?}"
    );
    let msg = relayed(
        &ask(
            port,
            9,
            "branch.switch",
            json!({ "repo": slug, "name": "side", "checkout": "wt-a" }),
        )
        .await,
    );
    assert_in_worktree(&msg, "branch.switch");
    assert!(
        msg.contains("branch switch -- side"),
        "switch argv unchanged; got {msg:?}"
    );

    // (g) An unknown name is refused BEFORE any spawn: the sentinel the child
    // writes on exit never appears.
    let sentinel = root.join("spawned.txt");
    std::env::set_var("RALPHY_TEST_DONE_FILE", &sentinel);
    for (id, checkout) in [(10u64, json!("nope")), (11, json!("")), (12, json!(5))] {
        let reply = ask(
            port,
            id,
            "changes.list",
            json!({ "repo": slug, "checkout": checkout }),
        )
        .await;
        assert_eq!(
            reply,
            json!({ "status": "error", "message": "unknown checkout" }),
            "checkout {checkout} must be refused as unknown"
        );
    }
    // The refusal reply arrives synchronously with the (absent) spawn; a
    // child that did run would have written the sentinel before exiting.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !sentinel.exists(),
        "an unknown checkout must spawn nothing (sentinel found at {sentinel:?})"
    );
    std::env::remove_var("RALPHY_TEST_DONE_FILE");
}
