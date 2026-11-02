use super::*;
use std::fs;

/// A generous window: the debounce settle plus slack for a slow CI host.
fn window() -> Duration {
    DEBOUNCE_TIMEOUT + Duration::from_secs(2)
}

async fn recv_in(rx: &mut DirtyRx, dur: Duration) -> Option<(String, String)> {
    tokio::time::timeout(dur, rx.recv())
        .await
        .ok()
        .and_then(Result::ok)
}

#[test]
fn an_access_event_changes_no_listing() {
    let read = notify::Event::new(EventKind::Access(notify::event::AccessKind::Any));
    assert!(changes_no_listing(&read));
}

#[tokio::test]
async fn storm_coalesces_to_few_nudges() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = WatcherManager::new(MAX_WATCHES);
    let mut rx = mgr.watch("owner/repo", dir.path(), "").unwrap();

    for i in 0..20 {
        fs::write(dir.path().join(format!("f{i}.txt")), b"x").unwrap();
    }

    // Drain until a quiet gap; a settled storm is one dir per batch, not 20.
    let mut count = 0;
    while recv_in(&mut rx, window()).await.is_some() {
        count += 1;
    }
    assert!((1..=3).contains(&count), "coalesced count = {count}");
}

#[tokio::test]
async fn unwatched_and_noise_children_emit_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = WatcherManager::new(MAX_WATCHES);
    let mut rx = mgr.watch("owner/repo", dir.path(), "").unwrap();

    // The two dirs are created UNDER the watch and their (legitimate)
    // root nudges drained to a quiet gap, so what follows is measured
    // against a settled stream. Created before the watch, FSEvents
    // replayed them into the receiver — its "since now" is coarser than
    // the gap between a `create_dir` and the watch — and the leg below
    // read a replayed `unwatched` as a nudge (macOS runner, 2026-09-17).
    fs::create_dir(dir.path().join("node_modules")).unwrap();
    fs::create_dir(dir.path().join("unwatched")).unwrap();
    while recv_in(&mut rx, window()).await.is_some() {}

    fs::write(dir.path().join("node_modules/x"), b"x").unwrap();
    fs::write(dir.path().join("unwatched/y"), b"y").unwrap();
    assert!(
        recv_in(&mut rx, window()).await.is_none(),
        "noise / unwatched children must not nudge"
    );

    fs::write(dir.path().join("visible.txt"), b"v").unwrap();
    assert_eq!(
        recv_in(&mut rx, window()).await,
        Some(("owner/repo".to_string(), String::new())),
        "a real child create nudges the root"
    );
}

/// Reading a watched dir the way the workbench does (`tree::list` opens the
/// dir and every child dir) must NOT nudge: on Linux those opens reach the
/// watcher as `Access` events, and a nudge for a read is answered with
/// another read — the loop that made a WSL project's tree flicker forever.
/// Vacuous on Windows (no open notifications there), load-bearing on Linux.
/// The trailing write proves the watch is alive, so silence was not a dead
/// watcher.
#[tokio::test]
async fn listing_a_watched_dir_emits_nothing() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("platform")).unwrap();
    fs::create_dir(dir.path().join("platform/apisix")).unwrap();
    fs::create_dir(dir.path().join("docs")).unwrap();

    let mgr = WatcherManager::new(MAX_WATCHES);
    let mut rx = mgr.watch("owner/repo", dir.path(), "").unwrap();
    let _rx_platform = mgr.watch("owner/repo", dir.path(), "platform").unwrap();
    // Let the watch SETUP settle first: opening a directory handle for the
    // OS watch can itself surface as a one-shot event on that dir (seen on
    // Windows), and this test is about the reads, not the arming.
    while recv_in(&mut rx, window()).await.is_some() {}

    for _ in 0..3 {
        crate::tree::list(dir.path(), "").unwrap();
        crate::tree::list(dir.path(), "platform").unwrap();
    }
    assert!(
        recv_in(&mut rx, window()).await.is_none(),
        "a read is not a change: listing must not nudge"
    );

    fs::write(dir.path().join("platform/new.yaml"), b"x").unwrap();
    assert_eq!(
        recv_in(&mut rx, window()).await,
        Some(("owner/repo".to_string(), "platform".to_string())),
        "a real write still nudges the dir it landed in"
    );
}

/// The amendment's (ADR-0036, 2026-07-26) watcher oracle: the tree lists
/// gitignored entries, so an ignored dir the operator expanded must refresh
/// too. Reds if the pump's gitignore filter comes back.
#[tokio::test]
async fn gitignored_dir_nudges_like_any_other() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(".gitignore"), b"ignored/\n").unwrap();
    fs::create_dir(dir.path().join("ignored")).unwrap();

    let mgr = WatcherManager::new(MAX_WATCHES);
    let mut rx = mgr.watch("owner/repo", dir.path(), "ignored").unwrap();

    fs::write(dir.path().join("ignored/y"), b"y").unwrap();
    assert_eq!(
        recv_in(&mut rx, window()).await,
        Some(("owner/repo".to_string(), "ignored".to_string())),
    );
}

#[tokio::test]
async fn runstate_nudges_despite_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(".gitignore"), b".ralphy/\n").unwrap();
    fs::create_dir_all(dir.path().join(RUNSTATE_REL)).unwrap();

    let mgr = WatcherManager::new(MAX_WATCHES);
    let mut rx = mgr.watch("owner/repo", dir.path(), RUNSTATE_REL).unwrap();

    fs::write(dir.path().join(RUNSTATE_REL).join("01ABC.json"), b"{}").unwrap();

    assert_eq!(
        recv_in(&mut rx, window()).await,
        Some(("owner/repo".to_string(), RUNSTATE_REL.to_string())),
        "the snapshot dir nudges even though `.ralphy/` is gitignored"
    );
}

#[test]
fn head_rels_are_the_standard_gitdirs_and_their_logs() {
    for rel in [
        ".git",
        ".git/logs",
        ".git/worktrees/wt-a",
        ".git/worktrees/wt-a/logs",
    ] {
        assert!(is_head_rel(rel), "{rel:?} is a HEAD rel");
    }
    for rel in [
        "",
        "src",
        ".git/refs",
        ".git/worktrees",
        ".git/worktrees/",
        ".git/worktrees/a/b",
        ".gitx",
    ] {
        assert!(!is_head_rel(rel), "{rel:?} is not a HEAD rel");
    }
}

// The loop guard: the nudge is answered with `git status`, which rewrites
// `index`, so only the `HEAD` child may pass.
#[tokio::test]
async fn a_head_rel_nudges_on_head_alone() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".git/logs")).unwrap();
    fs::write(dir.path().join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
    fs::write(dir.path().join(".git/logs/HEAD"), b"").unwrap();

    let mgr = WatcherManager::new(MAX_WATCHES);
    let mut rx = mgr.watch("owner/repo", dir.path(), ".git").unwrap();
    let _logs = mgr.watch("owner/repo", dir.path(), ".git/logs").unwrap();
    while recv_in(&mut rx, window()).await.is_some() {}

    fs::write(dir.path().join(".git/index"), b"i").unwrap();
    fs::write(dir.path().join(".git/index.lock"), b"i").unwrap();
    fs::remove_file(dir.path().join(".git/index.lock")).unwrap();
    assert_eq!(
        recv_in(&mut rx, window()).await,
        None,
        "index writes are not a HEAD move"
    );

    // What git does: write `HEAD.lock`, then rename it over `HEAD`.
    fs::write(dir.path().join(".git/HEAD.lock"), b"ref: refs/heads/x\n").unwrap();
    fs::rename(
        dir.path().join(".git/HEAD.lock"),
        dir.path().join(".git/HEAD"),
    )
    .unwrap();
    assert_eq!(
        recv_in(&mut rx, window()).await,
        Some(("owner/repo".to_string(), ".git".to_string())),
    );
    while recv_in(&mut rx, window()).await.is_some() {}

    let mut log = fs::OpenOptions::new()
        .append(true)
        .open(dir.path().join(".git/logs/HEAD"))
        .unwrap();
    std::io::Write::write_all(&mut log, b"a b c\n").unwrap();
    drop(log);
    assert_eq!(
        recv_in(&mut rx, window()).await,
        Some(("owner/repo".to_string(), ".git/logs".to_string())),
    );
}

#[tokio::test]
async fn refcount_shares_and_tears_down() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = WatcherManager::new(MAX_WATCHES);
    let _rx1 = mgr.watch("owner/repo", dir.path(), "").unwrap();
    let _rx2 = mgr.watch("owner/repo", dir.path(), "").unwrap();
    assert_eq!(
        mgr.watch_refcount("owner/repo", ""),
        2,
        "two share one watch"
    );

    mgr.unwatch("owner/repo", "");
    assert_eq!(mgr.watch_refcount("owner/repo", ""), 1);
    assert!(mgr.repo_active("owner/repo"), "one hold keeps the watcher");

    mgr.unwatch("owner/repo", "");
    assert!(
        !mgr.repo_active("owner/repo"),
        "the last release tears the repo watcher down"
    );
}

#[tokio::test]
async fn watch_rejects_parent_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = WatcherManager::new(MAX_WATCHES);
    assert!(
        mgr.watch("owner/repo", dir.path(), "../escape").is_err(),
        "a `..` path must be refused, never watched out-of-root"
    );
    assert!(
        !mgr.repo_active("owner/repo"),
        "a rejected watch creates no repo watcher"
    );
}

#[tokio::test]
async fn over_cap_degrades_to_poll() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("a")).unwrap();
    fs::create_dir(dir.path().join("b")).unwrap();

    let mgr = WatcherManager::new(1);
    let _rx_a = mgr.watch("owner/repo", dir.path(), "a").unwrap();
    let mut rx_b = mgr.watch("owner/repo", dir.path(), "b").unwrap();
    assert_eq!(
        mgr.mode("owner/repo"),
        Some(WatchMode::Poll),
        "a 2nd dir over max_watches=1 degrades to poll"
    );

    fs::write(dir.path().join("b/f.txt"), b"x").unwrap();
    let got = recv_in(&mut rx_b, POLL_INTERVAL + Duration::from_secs(2)).await;
    assert_eq!(
        got,
        Some(("owner/repo".to_string(), "b".to_string())),
        "poll still emits on a create"
    );
}

#[tokio::test]
async fn a_failed_first_watch_leaves_no_repo_watcher() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = WatcherManager::new(8);
    assert!(mgr.watch("owner/repo", dir.path(), "no-such-dir").is_err());
    assert!(!mgr.repo_active("owner/repo"));
}

/// A thread that panicked while it held the registry lock does not stop
/// later watches: each entry is changed in one step under the lock.
#[tokio::test]
async fn a_poisoned_registry_still_watches() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = Arc::new(WatcherManager::new(MAX_WATCHES));
    let holder = Arc::clone(&mgr);
    let panicked = std::thread::spawn(move || {
        let _held = holder.repos.lock();
        panic!("a panic while the registry is locked");
    })
    .join();
    assert!(panicked.is_err());
    assert!(mgr.repos.is_poisoned());

    let watched = mgr.watch("owner/repo", dir.path(), "");
    assert!(watched.is_ok(), "{:?}", watched.err());
    assert_eq!(mgr.watch_refcount("owner/repo", ""), 1);
    mgr.unwatch("owner/repo", "");
    assert!(!mgr.repo_active("owner/repo"));
}

#[tokio::test]
async fn a_failed_degrade_leaves_no_refcount() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("b")).unwrap();

    let mgr = WatcherManager::new(1);
    let _rx_root = mgr.watch("owner/repo", dir.path(), "").unwrap();
    mgr.fail_next_degrade
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(mgr.watch("owner/repo", dir.path(), "b").is_err());
    assert_eq!(mgr.watch_refcount("owner/repo", "b"), 0);
    assert_eq!(mgr.watch_refcount("owner/repo", ""), 1);
}
