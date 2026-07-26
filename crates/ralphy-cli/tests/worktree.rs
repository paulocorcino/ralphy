//! End-to-end coverage for `ralphy changes stage|unstage|commit` (issue #318):
//! drives the real `ralphy` binary against isolated temp git repos. Nothing here
//! names a remote, so none of it touches a network.
//!
//! The three held-lock tests are the oracle for "refuses under `HeldAlive`
//! BEFORE any git call": each captures the index and `HEAD` first and asserts
//! both are byte-identical afterwards, so a guard placed after the core call
//! would red them even though the exit code would look right.

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

/// A repo with one commit on `main`, plus an untracked `b.txt` to act on.
fn init_repo() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    run_git(dir.path(), &["init", "--quiet", "-b", "main"]);
    configure(dir.path());
    commit(dir.path(), "a.txt", "one\n", "init");
    std::fs::write(dir.path().join("b.txt"), "loose\n").unwrap();
    dir
}

fn ralphy(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args(args)
        .output()
        .expect("spawning ralphy")
}

/// Hold `repo`'s run lock with a live child, exactly as `tests/sync.rs` does.
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

/// The index and HEAD, as the two byte-exact values a refusal must not move.
fn git_state(repo: &Path) -> (String, String) {
    (
        git_output(repo, &["diff", "--cached", "--name-only"]),
        git_output(repo, &["rev-parse", "HEAD"]),
    )
}

#[test]
fn changes_stage_refuses_under_a_held_lock_before_any_git_call() {
    let repo = init_repo();
    let before = git_state(repo.path());

    let child = hold_run_lock(repo.path());
    let out = ralphy(&[
        "changes",
        "stage",
        "--repo",
        &repo.path().to_string_lossy(),
        "--path=b.txt",
    ]);
    release(child);

    assert!(
        !out.status.success(),
        "changes stage must refuse under a held run.lock"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refusing to changes stage"),
        "the refusal names the verb: {stderr}"
    );
    assert_eq!(
        git_state(repo.path()),
        before,
        "the guard runs BEFORE any git call: the index or HEAD moved"
    );
}

#[test]
fn changes_unstage_refuses_under_a_held_lock_before_any_git_call() {
    let repo = init_repo();
    run_git(repo.path(), &["add", "b.txt"]);
    let before = git_state(repo.path());
    assert_eq!(before.0, "b.txt", "the fixture really has a staged path");

    let child = hold_run_lock(repo.path());
    let out = ralphy(&[
        "changes",
        "unstage",
        "--repo",
        &repo.path().to_string_lossy(),
        "--path=b.txt",
    ]);
    release(child);

    assert!(
        !out.status.success(),
        "changes unstage must refuse under a held run.lock"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refusing to changes unstage"),
        "the refusal names the verb: {stderr}"
    );
    assert_eq!(
        git_state(repo.path()),
        before,
        "the guard runs BEFORE any git call: the index or HEAD moved"
    );
}

#[test]
fn changes_commit_refuses_under_a_held_lock_before_any_git_call() {
    let repo = init_repo();
    run_git(repo.path(), &["add", "b.txt"]);
    let before = git_state(repo.path());

    let child = hold_run_lock(repo.path());
    let out = ralphy(&[
        "changes",
        "commit",
        "--repo",
        &repo.path().to_string_lossy(),
        "--message=would land",
    ]);
    release(child);

    assert!(
        !out.status.success(),
        "changes commit must refuse under a held run.lock"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refusing to changes commit"),
        "the refusal names the verb: {stderr}"
    );
    assert_eq!(
        git_state(repo.path()),
        before,
        "the guard runs BEFORE any git call: the index or HEAD moved"
    );
}

/// The end-to-end oracle for the dash-safe token. MEASURED: git accepts
/// `-m -oops` on its own, so CLAP is the hop the fusion protects — a daemon
/// emitting `--message` and `-oops` as two tokens dies here with
/// `unexpected argument '-o' found`, exit 2, before any git call.
#[test]
fn commit_message_beginning_with_a_dash_is_committed_verbatim() {
    let repo = init_repo();

    let staged = ralphy(&[
        "changes",
        "stage",
        "--repo",
        &repo.path().to_string_lossy(),
        "--path=b.txt",
    ]);
    assert!(
        staged.status.success(),
        "changes stage must succeed: {}",
        String::from_utf8_lossy(&staged.stderr)
    );

    let out = ralphy(&[
        "changes",
        "commit",
        "--repo",
        &repo.path().to_string_lossy(),
        "--message=-oops",
    ]);
    assert!(
        out.status.success(),
        "a message beginning with `-` must commit: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        git_output(repo.path(), &["log", "-1", "--format=%s"]),
        "-oops",
        "the message is recorded verbatim, not read as a flag"
    );
}

/// A refusal the operator reads is the core's own prose, never git's — and it
/// leaves the repo exactly as it found it.
#[test]
fn changes_commit_with_nothing_staged_exits_non_zero_with_prose() {
    let repo = init_repo();
    let before = git_state(repo.path());

    let out = ralphy(&[
        "changes",
        "commit",
        "--repo",
        &repo.path().to_string_lossy(),
        "--message=nothing to record",
    ]);

    assert!(!out.status.success(), "an empty index must refuse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot commit: nothing is staged"),
        "the refusal reads as prose: {stderr}"
    );
    assert!(
        !stderr.contains("nothing added to commit"),
        "no git error string is relayed: {stderr}"
    );
    assert_eq!(git_state(repo.path()), before, "a refusal moves nothing");
}

/// The full round trip over the real binary: stage, unstage, stage, commit.
#[test]
fn changes_stage_then_unstage_round_trips_over_the_binary() {
    let repo = init_repo();
    let root = repo.path().to_string_lossy().to_string();

    assert!(
        ralphy(&["changes", "stage", "--repo", &root, "--path=b.txt"])
            .status
            .success()
    );
    assert_eq!(
        git_output(repo.path(), &["diff", "--cached", "--name-only"]),
        "b.txt"
    );

    assert!(
        ralphy(&["changes", "unstage", "--repo", &root, "--path=b.txt"])
            .status
            .success()
    );
    assert_eq!(
        git_output(repo.path(), &["diff", "--cached", "--name-only"]),
        "",
        "unstage emptied the index again"
    );
    assert!(
        repo.path().join("b.txt").exists(),
        "unstage touches the index, never the working tree"
    );

    // A path the change set does not name is refused by value, not by git.
    let refused = ralphy(&["changes", "stage", "--repo", &root, "--path=a.txt"]);
    assert!(!refused.status.success(), "an unchanged path must refuse");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("is not in the change set"),
        "the refusal is the core's prose: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
}
