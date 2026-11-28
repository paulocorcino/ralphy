//! How the `ralphy` binary reports an error: the failure form in the terminal, and one
//! JSON line on stderr under `--failure-format json`.

use std::process::Output;

use tempfile::TempDir;

use super::support::{commit, hold_run_lock, ralphy, release, run_git};

/// A repo with one commit and nothing staged.
fn clean_repo() -> TempDir {
    let dir = tempfile::tempdir().expect("creating a temporary directory");
    run_git(dir.path(), &["init", "--quiet", "-b", "main"]);
    run_git(dir.path(), &["config", "user.email", "test@example.com"]);
    run_git(dir.path(), &["config", "user.name", "Test"]);
    commit(dir.path(), "a.txt", "one\n", "init");
    dir
}

fn commit_nothing(repo: &TempDir, before: &[&str]) -> Output {
    let root = repo.path().to_string_lossy().to_string();
    let mut args = before.to_vec();
    args.extend(["changes", "commit", "--repo", &root, "--message=x"]);
    ralphy(&args)
}

/// The one stderr line, parsed.
fn json_line(out: &Output) -> serde_json::Value {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), 1, "exactly one stderr line: {stderr}");
    let line = lines.first().expect("the length is one");
    serde_json::from_str(line).expect("the line is JSON")
}

#[test]
fn a_failure_prints_cause_then_remedy() {
    let repo = clean_repo();
    let out = commit_nothing(&repo, &[]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines, ["error: nothing is staged", "Stage a file first."]);
}

#[test]
fn json_format_writes_one_failure_line() {
    let repo = clean_repo();
    let out = commit_nothing(&repo, &["--failure-format", "json"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "stdout is empty");
    let line = json_line(&out);
    let failure = &line["failure"];
    assert_eq!(failure["domain"], "working-tree");
    assert_eq!(failure["code"], "nothing-staged");
    assert_eq!(failure["text"]["cause"], "nothing is staged");
    assert_eq!(failure["text"]["remedy"], "Stage a file first.");
    let chain = line["chain"].as_str().expect("the chain is a string");
    assert!(!chain.is_empty(), "the chain is not empty");
}

#[test]
fn an_error_without_a_failure_is_ralphy_failed() {
    let not_a_repo = tempfile::tempdir().expect("creating a temporary directory");
    let root = not_a_repo.path().to_string_lossy().to_string();
    let out = ralphy(&[
        "--failure-format",
        "json",
        "changes",
        "commit",
        "--repo",
        &root,
        "--message=x",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let line = json_line(&out);
    let failure = &line["failure"];
    assert_eq!(failure["domain"], "daemon");
    assert_eq!(failure["code"], "ralphy-failed");
    let chain = line["chain"].as_str().expect("the chain is a string");
    assert!(!chain.is_empty(), "the chain is not empty");
    assert_eq!(failure["details"]["cause"], chain);
}

#[test]
fn a_commit_under_a_held_run_lock_is_run_holds_lock() {
    let repo = clean_repo();
    let lock = hold_run_lock(repo.path());
    let out = commit_nothing(&repo, &["--failure-format", "json"]);
    release(lock);
    assert_eq!(out.status.code(), Some(1));
    let line = json_line(&out);
    assert_eq!(line["failure"]["domain"], "working-tree");
    assert_eq!(line["failure"]["code"], "run-holds-lock");
    let chain = line["chain"].as_str().expect("the chain is a string");
    assert!(chain.contains("refusing to changes commit"), "{chain}");
}
