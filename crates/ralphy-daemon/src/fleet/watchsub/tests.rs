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
        vec![("owner/repo".to_string(), String::new())]
    );

    std::fs::write(dir.path().join("between.txt"), b"x").unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let buffered = subs.wait("s", window()).await;
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
