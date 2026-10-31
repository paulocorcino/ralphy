//! The `tree`, `runs` and `changes` channels.

use std::path::Path;
use std::time::Duration;

use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::registry;

use crate::support::{close, connect, next_inner, open, send_on, serve_with, Daemon, Ws};

const SLUG: &str = "owner/tree";

/// A daemon whose registry holds `SLUG` and a repo before it, both named for
/// the repo dir it returns: a push that echoes a constant or the first entry
/// fails the test that names `SLUG`.
async fn serve_repo() -> (Daemon, tempfile::TempDir) {
    let repo = tempfile::tempdir().expect("create the repo dir");
    let root = repo.path().to_string_lossy().into_owned();
    let daemon = serve_with(|store| {
        let mut entries = registry::RegistryStore::default();
        entries.upsert("owner/other", &root);
        entries.upsert(SLUG, &root);
        registry::save_to(&entries, &store.join("repos.toml")).expect("save the registry");
    })
    .await;
    (daemon, repo)
}

/// A client command as the inner frame of a channel.
fn command(verb: &str, payload: serde_json::Value) -> Vec<u8> {
    protocol::encode(&Frame::Command(Command {
        id: 0,
        verb: verb.to_string(),
        payload,
    }))
}

/// The payload of the next push named `verb` on `channel` within `within`.
async fn next_push(
    ws: &mut Ws,
    channel: u32,
    verb: &str,
    within: Duration,
) -> Option<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let inner = next_inner(ws, channel, left).await?;
        if let Ok(Frame::Command(pushed)) = protocol::decode(&inner) {
            if pushed.verb == verb {
                return Some(pushed.payload);
            }
        }
    }
}

/// Run `write` until the push named `verb` arrives on `channel`. The watch
/// has no reply, so the first writes may come before it is held; each round
/// writes again and waits a second.
async fn push_after_writes(
    ws: &mut Ws,
    channel: u32,
    verb: &str,
    mut write: impl FnMut(u32),
) -> Option<serde_json::Value> {
    for round in 0..10 {
        write(round);
        if let Some(payload) = next_push(ws, channel, verb, Duration::from_secs(1)).await {
            return Some(payload);
        }
    }
    None
}

/// Write a file in `dir`, which exists.
fn write_file(dir: &Path, round: u32) {
    std::fs::write(dir.join(format!("f{round}.json")), b"x").expect("write a file");
}

#[tokio::test]
async fn a_tree_channel_pushes_tree_dirty() {
    let (daemon, repo) = serve_repo().await;
    let mut ws = connect(&daemon).await;
    open(&mut ws, 1, "tree", serde_json::json!({})).await;
    let watch = command("watch", serde_json::json!({ "repo": SLUG, "path": "" }));
    send_on(&mut ws, 1, &watch).await;

    let pushed = push_after_writes(&mut ws, 1, "tree.dirty", |round| {
        write_file(repo.path(), round);
    })
    .await;

    assert_eq!(
        pushed,
        Some(serde_json::json!({ "repo": SLUG, "path": "" })),
        "a file written in the watched dir pushes tree.dirty on the tree channel"
    );
}

#[tokio::test]
async fn a_runs_channel_pushes_runs_dirty() {
    let (daemon, repo) = serve_repo().await;
    let mut ws = connect(&daemon).await;
    open(&mut ws, 2, "runs", serde_json::json!({})).await;
    // The runs verbs ignore the path; it is sent to show the dir is fixed.
    let watch = command(
        "runs.watch",
        serde_json::json!({ "repo": SLUG, "path": "elsewhere" }),
    );
    send_on(&mut ws, 2, &watch).await;
    let runstate = repo.path().join(".ralphy").join("runstate");

    let pushed = push_after_writes(&mut ws, 2, "runs.dirty", |round| {
        std::fs::create_dir_all(&runstate).expect("create the runstate dir");
        write_file(&runstate, round);
    })
    .await;

    assert_eq!(
        pushed,
        Some(serde_json::json!({ "repo": SLUG })),
        "a runstate write pushes runs.dirty on the runs channel"
    );
}

#[tokio::test]
async fn closing_a_tree_channel_releases_its_watches() {
    let (daemon, repo) = serve_repo().await;
    let mut ws = connect(&daemon).await;
    open(&mut ws, 1, "tree", serde_json::json!({})).await;
    let watch = command("watch", serde_json::json!({ "repo": SLUG, "path": "" }));
    send_on(&mut ws, 1, &watch).await;
    // The watch is held: without this the silence below proves nothing.
    let live = push_after_writes(&mut ws, 1, "tree.dirty", |round| {
        write_file(repo.path(), round);
    })
    .await;
    assert!(live.is_some(), "the watch pushes before the channel closes");

    close(&mut ws, 1).await;
    // Let the daemon take the close, and drop the pushes already sent.
    tokio::time::sleep(Duration::from_millis(500)).await;
    while next_inner(&mut ws, 1, Duration::from_millis(300))
        .await
        .is_some()
    {}

    write_file(repo.path(), 100);
    let after = next_push(&mut ws, 1, "tree.dirty", Duration::from_secs(2)).await;
    assert_eq!(after, None, "a closed channel pushes nothing");
}

#[expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]
fn use_the_test_child() {
    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "0");
    std::env::set_var("RALPHY_TEST_SLEEP_MS", "300");
}

#[tokio::test]
async fn a_changes_channel_pushes_changes_dirty_on_a_run_exit() {
    let (daemon, _repo) = serve_repo().await;
    use_the_test_child();
    let mut ws = connect(&daemon).await;
    open(&mut ws, 4, "changes", serde_json::json!({})).await;
    // The open is read before the run starts: the page socket reads in order,
    // and the run needs a connection of its own.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let run = Command {
        id: 1,
        verb: "run".to_string(),
        payload: serde_json::json!({ "repo": SLUG, "agent": "claude", "branchMode": "new" }),
    };
    let mut runner = connect(&daemon).await;
    open(
        &mut runner,
        1,
        "spawn",
        serde_json::json!({ "command": run }),
    )
    .await;

    let pushed = next_push(&mut ws, 4, "changes.dirty", Duration::from_secs(15)).await;

    assert_eq!(
        pushed,
        Some(serde_json::json!({ "repo": SLUG })),
        "a run exit pushes changes.dirty naming its repo on the changes channel"
    );
}
