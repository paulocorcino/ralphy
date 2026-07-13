//! End-to-end coverage for `ralphy branch switch|create` (ADR-0036 §6, issue
//! #189): drives the real `ralphy` binary against an isolated temp git repo,
//! never the checkout under test. `label set` needs `gh` (not in
//! `environment.md`) so only the guarded-refusal path — reached before any
//! forge call — is covered here.

use std::path::Path;
use std::process::Command;

/// `git init` a fresh temp repo with a born HEAD (an empty initial commit),
/// so branch creation/switch has a commit-ish to work from.
fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    run_git(root, &["init", "--quiet"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test"]);
    run_git(root, &["commit", "--allow-empty", "--quiet", "-m", "init"]);
    dir
}

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

#[test]
fn branch_create_makes_branch_when_lock_free() {
    let repo = init_repo();

    let status = Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args([
            "branch",
            "create",
            "feature-x",
            "--repo",
            &repo.path().to_string_lossy(),
        ])
        .status()
        .expect("spawning ralphy");
    assert!(
        status.success(),
        "branch create must succeed when lock is free"
    );

    let head = git_output(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(head, "feature-x");
}

#[test]
fn branch_switch_refuses_under_held_run_lock() {
    let repo = init_repo();

    let mut child = Command::new(env!("CARGO_BIN_EXE_runlock_test_child"))
        .spawn()
        .expect("spawning runlock_test_child");

    let lock_dir = repo.path().join(".ralphy");
    std::fs::create_dir_all(&lock_dir).unwrap();
    std::fs::write(
        lock_dir.join("run.lock"),
        serde_json::json!({
            "pid": child.id(),
            "started_at": "2026-07-13T10:00:00-03:00",
        })
        .to_string(),
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args([
            "branch",
            "switch",
            "other",
            "--repo",
            &repo.path().to_string_lossy(),
        ])
        .output()
        .expect("spawning ralphy");

    child.kill().ok();
    child.wait().ok();

    assert!(
        !out.status.success(),
        "branch switch must refuse under a held run.lock"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refusing to branch switch"),
        "stderr must explain the refusal, got: {stderr}"
    );
}
