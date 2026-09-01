use super::*;

fn window() -> Duration {
    Duration::from_secs(3)
}

#[tokio::test]
async fn create_emits_and_between_polls_stays_buffered() {
    let dir = tempfile::tempdir().unwrap();
    let watchers = Arc::new(watch::WatcherManager::new(watch::MAX_WATCHES));
    let subs = WatchSubs::new(watchers);
    subs.subscribe("s", "owner/repo", dir.path(), &[String::new()])
        .unwrap();

    std::fs::write(dir.path().join("first.txt"), b"x").unwrap();
    assert_eq!(
        subs.wait("s", window()).await,
        (
            vec![("owner/repo".to_string(), String::new())],
            WaitOutcome::Dirty
        )
    );

    std::fs::write(dir.path().join("between.txt"), b"x").unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (buffered, _) = subs.wait("s", window()).await;
    assert!(
        !buffered.is_empty()
            && buffered
                .iter()
                .all(|item| item == &("owner/repo".to_string(), String::new())),
        "an event between polls must remain in the held receiver: {buffered:?}"
    );
}

#[tokio::test]
async fn close_and_sweep_release_every_watch() {
    let dir = tempfile::tempdir().unwrap();
    let watchers = Arc::new(watch::WatcherManager::new(watch::MAX_WATCHES));
    let subs = WatchSubs::new(watchers.clone());
    subs.subscribe("close", "owner/repo", dir.path(), &[String::new()])
        .unwrap();
    assert_eq!(watchers.watch_refcount("owner/repo", ""), 1);
    subs.close("close");
    assert_eq!(watchers.watch_refcount("owner/repo", ""), 0);
    assert!(!watchers.repo_active("owner/repo"));

    subs.subscribe("sweep", "owner/repo", dir.path(), &[String::new()])
        .unwrap();
    assert_eq!(watchers.watch_refcount("owner/repo", ""), 1);
    tokio::time::sleep(Duration::from_millis(1)).await;
    subs.sweep(Duration::ZERO);
    assert_eq!(watchers.watch_refcount("owner/repo", ""), 0);
    assert!(!watchers.repo_active("owner/repo"));
}

#[tokio::test]
async fn traversal_is_refused_without_creating_a_sub() {
    let dir = tempfile::tempdir().unwrap();
    let watchers = Arc::new(watch::WatcherManager::new(watch::MAX_WATCHES));
    let subs = WatchSubs::new(watchers.clone());
    assert!(subs
        .subscribe(
            "escape",
            "owner/repo",
            dir.path(),
            &["../escape".to_string()]
        )
        .is_err());
    assert!(subs.take_buffered("escape").is_empty());
    assert!(!watchers.repo_active("owner/repo"));
}

/// TWO subscriptions on ONE repo is the shape that matters: the nudge broadcast
/// is per-repo, so the root subscription's events reach the docs subscription's
/// receiver. Ending the wait on one of those was what turned every unrelated
/// write into a full poll round trip — and, on the peer transport, into an
/// ephemeral port held for four minutes (2026-09-01).
#[tokio::test]
async fn an_unmatched_nudge_does_not_end_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("docs")).unwrap();
    let watchers = Arc::new(watch::WatcherManager::new(watch::MAX_WATCHES));
    let subs = WatchSubs::new(watchers);
    subs.subscribe("root", "owner/repo", dir.path(), &[String::new()])
        .unwrap();
    subs.subscribe("docs", "owner/repo", dir.path(), &["docs".to_string()])
        .unwrap();

    std::fs::write(dir.path().join("at-root.txt"), b"x").unwrap();
    let deadline = Duration::from_millis(1200);
    let started = Instant::now();
    let (dirty, outcome) = subs.wait("docs", deadline).await;

    assert!(
        dirty.is_empty(),
        "a root write is not a docs change: {dirty:?}"
    );
    assert_eq!(outcome, WaitOutcome::Timeout);
    assert!(
        started.elapsed() >= deadline - Duration::from_millis(50),
        "the wait must hold its deadline, not answer on another sub's nudge (held {:?})",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_matched_nudge_after_an_unmatched_one_is_delivered() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("docs")).unwrap();
    let watchers = Arc::new(watch::WatcherManager::new(watch::MAX_WATCHES));
    let subs = WatchSubs::new(watchers);
    subs.subscribe("root", "owner/repo", dir.path(), &[String::new()])
        .unwrap();
    subs.subscribe("docs", "owner/repo", dir.path(), &["docs".to_string()])
        .unwrap();

    let root = dir.path().to_path_buf();
    let writer = tokio::spawn(async move {
        std::fs::write(root.join("at-root.txt"), b"x").unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        std::fs::write(root.join("docs").join("page.md"), b"x").unwrap();
    });
    let (dirty, outcome) = subs.wait("docs", Duration::from_secs(10)).await;
    writer.await.unwrap();

    assert_eq!(outcome, WaitOutcome::Dirty);
    assert!(
        !dirty.is_empty()
            && dirty
                .iter()
                .all(|item| item == &("owner/repo".to_string(), "docs".to_string())),
        "skipping the root nudge must not lose the docs one that followed: {dirty:?}"
    );
}

/// The invariant the whole poll route rests on: there is NO input that makes a
/// wait answer fast and empty. A subscription that vanished (swept, closed) is
/// the last way one could, and the caller would re-post immediately.
#[tokio::test]
async fn a_wait_on_an_unknown_sub_still_holds_its_deadline() {
    let watchers = Arc::new(watch::WatcherManager::new(watch::MAX_WATCHES));
    let subs = WatchSubs::new(watchers);
    let deadline = Duration::from_millis(400);
    let started = Instant::now();
    let (dirty, outcome) = subs.wait("nobody", deadline).await;

    assert!(dirty.is_empty());
    assert_eq!(outcome, WaitOutcome::NoSub);
    assert!(
        started.elapsed() >= deadline - Duration::from_millis(50),
        "an unknown sub must not answer fast (held {:?})",
        started.elapsed()
    );
}
