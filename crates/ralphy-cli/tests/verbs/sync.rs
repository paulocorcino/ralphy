//! End-to-end coverage for `ralphy sync status|fetch|pull` (issue #316): drives
//! the real `ralphy` binary against isolated temp git repos. The "remote" is
//! always a LOCAL directory cloned by path, so nothing here touches a network.
//!
//! The JSON shape asserted here is the wire contract the daemon's `sync.status`
//! verb consumes.
//!
//! The held-lock refusals of `sync fetch` and `sync pull` are rows of
//! `lock_refusal.rs`; [`lock_rows`] builds them.

use std::path::Path;

use tempfile::TempDir;

use super::support::{commit, git_output, hold_run_lock, ralphy, release, run_git, LockRow};

fn configure(root: &Path) {
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test"]);
}

/// A repo with one commit on `main` and no remote of its own.
fn init_remote() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    run_git(dir.path(), &["init", "--quiet", "-b", "main"]);
    configure(dir.path());
    commit(dir.path(), "a.txt", "one\n", "init");
    dir
}

/// A clone of `remote` by filesystem path.
fn clone_of(remote: &Path) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    run_git(
        dir.path(),
        &[
            "clone",
            "--quiet",
            &remote.to_string_lossy(),
            &dir.path().to_string_lossy(),
        ],
    );
    configure(dir.path());
    dir
}

#[test]
fn sync_status_json_shape_is_the_wire_contract() {
    let remote = init_remote();
    let clone = clone_of(remote.path());
    commit(remote.path(), "b.txt", "two\n", "second");
    run_git(clone.path(), &["fetch", "--quiet"]);

    let out = ralphy(&[
        "sync",
        "status",
        "--format",
        "json",
        "--repo",
        &clone.path().to_string_lossy(),
    ]);
    assert!(out.status.success(), "sync status must succeed");

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("sync status emits JSON");
    assert_eq!(v["sync"]["head"]["kind"], "branch");
    assert_eq!(v["sync"]["head"]["name"], "main");
    assert_eq!(v["sync"]["tracking"]["upstream"], "origin/main");
    assert_eq!(v["sync"]["tracking"]["ahead"], 0);
    assert_eq!(v["sync"]["tracking"]["behind"], 1);
    assert!(
        v["sync"]["last_fetch"].is_string(),
        "a fetched repo stamps when: {v}"
    );
    super::golden::check(
        "sync.status",
        serde_json::json!({ "status": "ok", "sync": v }),
        &["/sync/sync/last_fetch"],
    );
}

/// The absent-upstream state must cross the wire as `null`, not as a zeroed
/// object — the UI cannot render "no upstream" from counts that read `0/0`.
#[test]
fn sync_status_no_upstream_is_null_not_zero() {
    let repo = init_remote();

    let out = ralphy(&[
        "sync",
        "status",
        "--format",
        "json",
        "--repo",
        &repo.path().to_string_lossy(),
    ]);
    assert!(out.status.success(), "sync status must succeed");

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("sync status emits JSON");
    assert_eq!(v["sync"]["head"]["kind"], "branch");
    assert!(
        v["sync"]["tracking"].is_null(),
        "no upstream must be null: {v}"
    );
    assert!(v["sync"]["last_fetch"].is_null(), "never fetched: {v}");
    super::golden::check(
        "sync.status--no-upstream",
        serde_json::json!({ "status": "ok", "sync": v }),
        &[],
    );
}

/// A detached HEAD crosses the wire with its short sha, which the UI shows in
/// place of a branch name.
#[test]
fn sync_status_detached_carries_the_sha() {
    let remote = init_remote();
    let clone = clone_of(remote.path());
    run_git(clone.path(), &["switch", "--detach", "HEAD"]);
    let short = git_output(clone.path(), &["rev-parse", "--short", "HEAD"]);

    let out = ralphy(&[
        "sync",
        "status",
        "--format",
        "json",
        "--repo",
        &clone.path().to_string_lossy(),
    ]);
    assert!(out.status.success(), "sync status must succeed");

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("sync status emits JSON");
    assert_eq!(v["sync"]["head"]["kind"], "detached", "got {v}");
    assert_eq!(v["sync"]["head"]["sha"], short.as_str(), "got {v}");
    let mut volatile = vec!["/sync/sync/head/sha"];
    if !v["sync"]["last_fetch"].is_null() {
        volatile.push("/sync/sync/last_fetch");
    }
    super::golden::check(
        "sync.status--detached",
        serde_json::json!({ "status": "ok", "sync": v }),
        &volatile,
    );
}

/// The DEFAULT output — the one a terminal operator sees — must carry the same
/// distinction the JSON does: an absent upstream reads as its own words, and a
/// detached HEAD as its own, never as zeroed counts.
#[test]
fn sync_status_without_format_never_prints_zeroed_counts_for_a_stateless_head() {
    let repo = init_remote();
    let noup = String::from_utf8_lossy(
        &ralphy(&["sync", "status", "--repo", &repo.path().to_string_lossy()]).stdout,
    )
    .trim()
    .to_string();
    assert!(
        noup.contains("no upstream"),
        "the human line names the state: {noup:?}"
    );
    assert!(
        !noup.contains("ahead") && !noup.contains("behind"),
        "a branch with no upstream has no counts to print: {noup:?}"
    );

    run_git(repo.path(), &["checkout", "--quiet", "--detach", "HEAD"]);
    let detached = String::from_utf8_lossy(
        &ralphy(&["sync", "status", "--repo", &repo.path().to_string_lossy()]).stdout,
    )
    .trim()
    .to_string();
    assert!(
        detached.contains("detached at"),
        "the human line names the state: {detached:?}"
    );
    assert!(
        !detached.contains("ahead") && !detached.contains("behind"),
        "a detached HEAD has no counts to print: {detached:?}"
    );

    // The positive control: a branch that DOES track prints its counts, so the
    // two assertions above are not satisfied by an empty line.
    let remote = init_remote();
    let clone = clone_of(remote.path());
    let tracking = String::from_utf8_lossy(
        &ralphy(&["sync", "status", "--repo", &clone.path().to_string_lossy()]).stdout,
    )
    .trim()
    .to_string();
    assert!(
        tracking.contains("origin/main") && tracking.contains("0 ahead, 0 behind"),
        "a tracking branch prints its counts: {tracking:?}"
    );
}

/// A read never blocks on the run lock — the workbench keeps rendering the
/// counts while a run holds the repo.
#[test]
fn sync_status_reads_under_a_held_lock() {
    let remote = init_remote();
    let clone = clone_of(remote.path());

    let child = hold_run_lock(clone.path());
    let out = ralphy(&[
        "sync",
        "status",
        "--format",
        "json",
        "--repo",
        &clone.path().to_string_lossy(),
    ]);
    release(child);

    assert!(
        out.status.success(),
        "sync status must read under a held lock: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("sync status emits JSON");
    assert_eq!(v["sync"]["tracking"]["upstream"], "origin/main");
}

/// The refusal the operator reads is the core's own prose, never git's.
#[test]
fn sync_pull_diverged_exits_non_zero_with_prose() {
    let remote = init_remote();
    let clone = clone_of(remote.path());
    commit(remote.path(), "b.txt", "theirs\n", "theirs");
    commit(clone.path(), "c.txt", "ours\n", "ours");
    run_git(clone.path(), &["fetch", "--quiet"]);
    let before = git_output(clone.path(), &["rev-parse", "HEAD"]);

    let out = ralphy(&["sync", "pull", "--repo", &clone.path().to_string_lossy()]);

    assert!(!out.status.success(), "a diverged branch must refuse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot fast-forward") && stderr.contains("have diverged"),
        "the refusal reads as prose: {stderr}"
    );
    assert!(
        !stderr.contains("fatal:"),
        "no git error string is relayed: {stderr}"
    );
    assert_eq!(
        git_output(clone.path(), &["rev-parse", "HEAD"]),
        before,
        "a refusal moves nothing"
    );
}

#[test]
fn sync_fetch_then_pull_fast_forwards() {
    let remote = init_remote();
    let clone = clone_of(remote.path());
    commit(remote.path(), "b.txt", "two\n", "second");

    let fetched = ralphy(&["sync", "fetch", "--repo", &clone.path().to_string_lossy()]);
    assert!(
        fetched.status.success(),
        "sync fetch must succeed: {}",
        String::from_utf8_lossy(&fetched.stderr)
    );

    let pulled = ralphy(&["sync", "pull", "--repo", &clone.path().to_string_lossy()]);
    assert!(
        pulled.status.success(),
        "sync pull must fast-forward: {}",
        String::from_utf8_lossy(&pulled.stderr)
    );
    assert!(
        String::from_utf8_lossy(&pulled.stdout).contains("Fast-forwarded 1"),
        "the success line counts the commits: {:?}",
        String::from_utf8_lossy(&pulled.stdout)
    );
    assert!(
        clone.path().join("b.txt").exists(),
        "the fast-forward landed on disk"
    );
}

/// The held-lock rows of `sync fetch` (its state: whether `FETCH_HEAD` exists)
/// and `sync pull` (its state: `HEAD`). Each clone's remote has a commit the
/// clone lacks, so an unguarded verb would move that state.
pub(super) fn lock_rows() -> Vec<LockRow> {
    let fetch_remote = init_remote();
    let fetch_clone = clone_of(fetch_remote.path());
    commit(
        fetch_remote.path(),
        "b.txt",
        "two
",
        "second",
    );
    let fetch_head = fetch_clone.path().join(".git").join("FETCH_HEAD");
    assert!(!fetch_head.exists(), "a fresh clone leaves no FETCH_HEAD");

    let pull_remote = init_remote();
    let pull_clone = clone_of(pull_remote.path());
    commit(
        pull_remote.path(),
        "b.txt",
        "two
",
        "second",
    );
    run_git(pull_clone.path(), &["fetch", "--quiet"]);
    let pull_root = pull_clone.path().to_path_buf();

    vec![
        LockRow {
            verb: "sync fetch",
            lock_repo: fetch_clone.path().to_path_buf(),
            args: sync_args("fetch", fetch_clone.path()),
            state: Box::new(move || format!("FETCH_HEAD exists: {}", fetch_head.exists())),
            _dirs: vec![fetch_remote, fetch_clone],
        },
        LockRow {
            verb: "sync pull",
            lock_repo: pull_root.clone(),
            args: sync_args("pull", &pull_root),
            state: Box::new(move || git_output(&pull_root, &["rev-parse", "HEAD"])),
            _dirs: vec![pull_remote, pull_clone],
        },
    ]
}

fn sync_args(verb: &str, repo: &Path) -> Vec<String> {
    vec![
        "sync".to_string(),
        verb.to_string(),
        "--repo".to_string(),
        repo.to_string_lossy().to_string(),
    ]
}
