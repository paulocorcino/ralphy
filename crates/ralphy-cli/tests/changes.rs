//! End-to-end coverage for `ralphy changes list` (issue #307): drives the real
//! `ralphy` binary against an isolated temp git repo. The JSON shape asserted
//! here is the wire contract the daemon's `changes.list` verb consumes.

use std::path::Path;
use std::process::Command;

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    run_git(root, &["init", "--quiet"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("README.md"), "changed\n").unwrap();
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

#[test]
fn changes_list_json_shape_is_the_wire_contract() {
    let repo = init_repo();

    let out = Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args([
            "changes",
            "list",
            "--format",
            "json",
            "--repo",
            &repo.path().to_string_lossy(),
        ])
        .output()
        .expect("spawning ralphy");
    assert!(out.status.success(), "changes list must succeed");

    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("changes list emits JSON");
    let list = v["changes"].as_array().expect("changes array");
    assert_eq!(list.len(), 1, "one edited file: {list:?}");
    assert_eq!(v["changes"][0]["path"], "README.md");
    assert_eq!(v["changes"][0]["status"], "modified");
    assert!(
        v["changes"][0]["original_path"].is_null(),
        "a plain edit carries no original path"
    );
}

#[test]
fn changes_list_without_format_prints_one_entry_per_line() {
    let repo = init_repo();

    let out = Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args(["changes", "list", "--repo", &repo.path().to_string_lossy()])
        .output()
        .expect("spawning ralphy");
    assert!(out.status.success(), "changes list must succeed");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec!["modified README.md"],
        "plain output: {stdout:?}"
    );
}
