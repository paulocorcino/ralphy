//! The CLI semantics the daemon's cwd hand-off relies on (issue #407; ADR-0063
//! §2): a git-backed `ralphy` verb run with a linked worktree as its cwd — no
//! `--repo` — acts on THAT worktree's index, working tree and HEAD and leaves
//! the primary untouched, while the `.ralphy/run.lock` gate stays the
//! primary's. This was green before #407 by design: the daemon changes nothing
//! in the CLI, it only picks the cwd; this file pins what that cwd buys.
//!
//! Drives the real `ralphy` binary against one isolated temp repo (the suite
//! is spawn-bound, so the legs are sequential in one test). Nothing names a
//! remote, so none of it touches a network. The worktree is made through
//! `ralphy worktree add`, so the pointer file is production-shaped.

use std::path::{Path, PathBuf};
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

fn head_branch(root: &Path) -> String {
    git_output(root, &["rev-parse", "--abbrev-ref", "HEAD"])
}

fn last_subject(root: &Path) -> String {
    git_output(root, &["log", "-1", "--format=%s"])
}

fn configure(root: &Path) {
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test"]);
    // LF in the blob and on disk, so the `blob read` oracle is byte-exact.
    run_git(root, &["config", "core.autocrlf", "false"]);
}

fn ralphy(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args(args)
        .output()
        .expect("spawning ralphy")
}

/// The daemon's spawn shape: the verb with `cwd` as `current_dir` and no
/// `--repo` — the cwd alone says which tree.
fn ralphy_in(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawning ralphy")
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// A repo on `main` with `a.txt` = `one\n` (commit `init`), a `side` branch,
/// and one linked worktree `wt-a` (on branch `wt-a`) made by the CLI.
fn init_repo() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    run_git(root, &["init", "--quiet", "-b", "main"]);
    configure(root);
    std::fs::write(root.join(".gitignore"), ".ralphy/\n").unwrap();
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    run_git(root, &["branch", "side"]);
    let out = ralphy(&["worktree", "add", "wt-a", "--repo", &root.to_string_lossy()]);
    assert!(
        out.status.success(),
        "worktree add must succeed: {}",
        stderr(&out)
    );
    let wt = root.join(".ralphy").join("worktrees").join("wt-a");
    assert!(wt.join(".git").is_file(), "the CLI wrote the pointer file");
    (dir, wt)
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

#[test]
fn git_backed_verbs_run_in_the_worktree_cwd_act_on_the_worktree_only() {
    let (dir, wt) = init_repo();
    let root = dir.path();

    // (1) changes list: the worktree's status, and the primary's is clean.
    std::fs::write(wt.join("a.txt"), "two\n").unwrap();
    let out = ralphy_in(&wt, &["changes", "list", "--format", "json"]);
    assert!(out.status.success(), "changes list in wt: {}", stderr(&out));
    assert!(
        stdout(&out).contains("\"path\":\"a.txt\""),
        "the worktree's modified a.txt must be listed; got {}",
        stdout(&out)
    );
    let out = ralphy_in(root, &["changes", "list", "--format", "json"]);
    assert!(
        out.status.success(),
        "changes list in root: {}",
        stderr(&out)
    );
    assert_eq!(
        stdout(&out),
        r#"{"changes":[]}"#,
        "the primary sees no change from the worktree's edit"
    );

    // (2) stage + commit land on the worktree's branch, never the primary's.
    let out = ralphy_in(&wt, &["changes", "stage", "--path=a.txt"]);
    assert!(out.status.success(), "stage in wt: {}", stderr(&out));
    let out = ralphy_in(&wt, &["changes", "commit", "--message=wt"]);
    assert!(out.status.success(), "commit in wt: {}", stderr(&out));
    assert_eq!(last_subject(&wt), "wt");
    assert_eq!(
        last_subject(root),
        "init",
        "the primary's HEAD did not move"
    );
    assert_eq!(head_branch(root), "main");
    assert_eq!(head_branch(&wt), "wt-a");

    // (3) blob read: each tree's OWN HEAD blob.
    let blob_at = |cwd: &Path| -> String {
        let out = ralphy_in(
            cwd,
            &[
                "blob",
                "read",
                "--revision",
                "head",
                "--path",
                "a.txt",
                "--format",
                "json",
            ],
        );
        assert!(
            out.status.success(),
            "blob read in {cwd:?}: {}",
            stderr(&out)
        );
        let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json reply");
        v["content"]
            .as_str()
            .unwrap_or_else(|| panic!("a content string; got {v}"))
            .to_string()
    };
    assert_eq!(
        blob_at(&wt),
        "two\n",
        "the worktree's HEAD carries the commit"
    );
    assert_eq!(blob_at(root), "one\n", "the primary's HEAD is still init");

    // (4) branch switch moves the worktree's HEAD and no other tree's.
    let out = ralphy_in(&wt, &["branch", "switch", "side"]);
    assert!(out.status.success(), "switch side in wt: {}", stderr(&out));
    assert_eq!(head_branch(&wt), "side");
    assert_eq!(head_branch(root), "main");

    // (5) A branch checked out in the primary is refused by git for the
    // worktree; both HEADs stay put.
    let out = ralphy_in(&wt, &["branch", "switch", "main"]);
    assert!(
        !out.status.success(),
        "switching the worktree to the primary's branch must fail"
    );
    let err = stderr(&out);
    assert!(
        err.contains("already"),
        "git's refusal names the other tree; got {err:?}"
    );
    assert_eq!(head_branch(&wt), "side");
    assert_eq!(head_branch(root), "main");

    // (6) The run lock is the PRIMARY's: held there, it gates a write in the
    // primary and NOT one in the worktree.
    let lock = hold_run_lock(root);
    std::fs::write(wt.join("a.txt"), "three\n").unwrap();
    let out = ralphy_in(&wt, &["changes", "stage", "--path=a.txt"]);
    assert!(
        out.status.success(),
        "stage in wt under the primary's lock: {}",
        stderr(&out)
    );
    assert_eq!(
        git_output(&wt, &["diff", "--cached", "--name-only"]),
        "a.txt",
        "the worktree's index took the stage"
    );
    std::fs::write(root.join("b.txt"), "loose\n").unwrap();
    let out = ralphy_in(root, &["changes", "stage", "--path=b.txt"]);
    release(lock);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stage in the primary under its own lock exits 1; stderr {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("refusing to changes stage"),
        "the refusal prose; got {}",
        stderr(&out)
    );
    assert_eq!(
        git_output(root, &["diff", "--cached", "--name-only"]),
        "",
        "the primary's index took nothing"
    );
}
