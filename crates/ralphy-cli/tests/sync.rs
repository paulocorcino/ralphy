//! End-to-end coverage for `ralphy sync status|fetch|pull` (issue #316): drives
//! the real `ralphy` binary against isolated temp git repos. The "remote" is
//! always a LOCAL directory cloned by path, so nothing here touches a network.
//!
//! The JSON shape asserted here is the wire contract the daemon's `sync.status`
//! verb consumes.

use std::path::Path;
use std::process::{Child, Command};

use tempfile::TempDir;

fn run_git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .expect("spawning git");
    assert!(status.success(), "git {args:?} failed");
}

fn git_output(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("spawning git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn configure(root: &Path) {
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test"]);
}

fn commit(root: &Path, file: &str, body: &str, msg: &str) {
    std::fs::write(root.join(file), body).unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "--quiet", "-m", msg]);
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

fn ralphy(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args(args)
        .output()
        .expect("spawning ralphy")
}

/// Hold `repo`'s run lock with a live child, exactly as `tests/mutate.rs` does.
fn hold_run_lock(repo: &Path) -> Child {
    let child = Command::new(env!("CARGO_BIN_EXE_runlock_test_child"))
        .spawn()
        .expect("spawning runlock_test_child");
    let lock_dir = repo.join(".ralphy");
    std::fs::create_dir_all(&lock_dir).unwrap();
    std::fs::write(
        lock_dir.join("run.lock"),
        serde_json::json!({
            "pid": child.id(),
            "started_at": "2026-07-25T10:00:00-03:00",
        })
        .to_string(),
    )
    .unwrap();
    child
}

fn release(mut child: Child) {
    child.kill().ok();
    child.wait().ok();
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
}

#[test]
fn sync_fetch_refuses_under_a_held_lock_before_any_git_call() {
    let remote = init_remote();
    let clone = clone_of(remote.path());
    commit(remote.path(), "b.txt", "two\n", "second");
    let fetch_head = clone.path().join(".git").join("FETCH_HEAD");
    assert!(!fetch_head.exists(), "a fresh clone leaves no FETCH_HEAD");

    let child = hold_run_lock(clone.path());
    let out = ralphy(&["sync", "fetch", "--repo", &clone.path().to_string_lossy()]);
    release(child);

    assert!(
        !out.status.success(),
        "sync fetch must refuse under a held run.lock"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refusing to sync fetch"),
        "the refusal names the verb: {stderr}"
    );
    assert!(
        !fetch_head.exists(),
        "the guard runs BEFORE any git call: FETCH_HEAD appeared"
    );
}

#[test]
fn sync_pull_refuses_under_a_held_lock_before_any_git_call() {
    let remote = init_remote();
    let clone = clone_of(remote.path());
    commit(remote.path(), "b.txt", "two\n", "second");
    run_git(clone.path(), &["fetch", "--quiet"]);
    let before = git_output(clone.path(), &["rev-parse", "HEAD"]);

    let child = hold_run_lock(clone.path());
    let out = ralphy(&["sync", "pull", "--repo", &clone.path().to_string_lossy()]);
    release(child);

    assert!(
        !out.status.success(),
        "sync pull must refuse under a held run.lock"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refusing to sync pull"),
        "the refusal names the verb: {stderr}"
    );
    assert_eq!(
        git_output(clone.path(), &["rev-parse", "HEAD"]),
        before,
        "the guard runs BEFORE any git call: HEAD moved"
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
