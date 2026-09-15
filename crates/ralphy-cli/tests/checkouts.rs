//! End-to-end coverage for `ralphy worktree list` (ADR-0063 §1, issue #403):
//! drives the real `ralphy` binary against ONE isolated temp git repo holding a
//! single workbench worktree — never the checkout under test. (`tests/worktree.rs`
//! is the working-tree *changes* suite; this file is the checkouts one.)

use std::path::Path;
use std::process::Command;

/// `git init` a fresh temp repo with a born HEAD (an empty initial commit)
/// and one workbench worktree `wt-a` under `.ralphy/worktrees/`, dirtied with
/// an untracked file so the human form's suffix is exercised.
fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    run_git(root, &["init", "--quiet"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test"]);
    run_git(root, &["commit", "--allow-empty", "--quiet", "-m", "init"]);
    std::fs::create_dir_all(root.join(".ralphy/worktrees")).unwrap();
    run_git(
        root,
        &[
            "worktree",
            "add",
            "--no-track",
            "-b",
            "wt-a",
            ".ralphy/worktrees/wt-a",
            "HEAD",
        ],
    );
    std::fs::write(root.join(".ralphy/worktrees/wt-a/scratch.txt"), "dirty\n").unwrap();
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

fn ralphy(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args(args)
        .output()
        .expect("spawning ralphy")
}

#[test]
fn worktree_list_prints_json_and_a_starred_primary() {
    let repo = init_repo();
    let root = repo.path().to_string_lossy().to_string();

    let out = ralphy(&["worktree", "list", "--format", "json", "--repo", &root]);
    assert!(
        out.status.success(),
        "worktree list --format json must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert_eq!(
        v["primary"],
        git_output(repo.path(), &["rev-parse", "--show-toplevel"]),
        "primary is git's own spelling of the toplevel"
    );
    let worktrees = v["worktrees"].as_array().expect("worktrees is an array");
    assert_eq!(worktrees.len(), 1, "got: {v}");
    assert_eq!(worktrees[0]["name"], "wt-a");
    assert_eq!(worktrees[0]["branch"], "wt-a");
    assert_eq!(worktrees[0]["base"], "");
    assert_eq!(worktrees[0]["dirty"], true, "scratch.txt dirties wt-a");
    assert!(
        worktrees[0]["path"]
            .as_str()
            .unwrap()
            .ends_with("/.ralphy/worktrees/wt-a"),
        "got: {v}"
    );

    // `--repo` pointing INSIDE the worktree lists the same trees.
    let from_inside = ralphy(&[
        "worktree",
        "list",
        "--format",
        "json",
        "--repo",
        &format!("{root}/.ralphy/worktrees/wt-a"),
    ]);
    assert!(
        from_inside.status.success(),
        "listing from inside a worktree"
    );
    let inner: serde_json::Value = serde_json::from_slice(&from_inside.stdout).unwrap();
    assert_eq!(inner, v, "the listing is the same from the worktree");

    let out = ralphy(&["worktree", "list", "--repo", &root]);
    assert!(out.status.success(), "plain worktree list must succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.first().copied(),
        Some(format!("* {}", v["primary"].as_str().unwrap()).as_str()),
        "the first line stars the primary: {stdout}"
    );
    assert_eq!(
        lines.get(1).copied(),
        Some("  wt-a  wt-a  (uncommitted changes)"),
        "one exact row per worktree, dirty suffix included: {stdout}"
    );
    assert_eq!(lines.len(), 2, "nothing after the rows: {stdout}");
}
