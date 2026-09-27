//! End-to-end coverage for `ralphy changes stage|unstage|commit|discard`
//! (issues #318, #319): drives the real `ralphy` binary against isolated temp
//! git repos. Nothing here names a remote, so none of it touches a network.
//!
//! The held-lock refusals of these four verbs are rows of
//! `lock_refusal.rs`; [`lock_rows`] builds them.

use std::path::Path;

use tempfile::TempDir;

use super::support::{commit, git_output, ralphy, run_git, LockRow};

fn configure(root: &Path) {
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test"]);
    // Without this, this host leaves LF in the blob and CRLF on disk, and the
    // discard tests' exact-content oracle becomes a coin flip.
    run_git(root, &["config", "core.autocrlf", "false"]);
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

/// The index and HEAD, as the two byte-exact values a refusal must not move.
fn git_state(repo: &Path) -> (String, String) {
    (
        git_output(repo, &["diff", "--cached", "--name-only"]),
        git_output(repo, &["rev-parse", "HEAD"]),
    )
}

/// The success leg: both cases in one call, then the refusal that proves the
/// change set really was re-read afterwards.
#[test]
fn changes_discard_over_the_binary_restores_and_deletes() {
    let repo = init_repo();
    let root = repo.path().to_string_lossy().to_string();
    std::fs::write(repo.path().join("a.txt"), "mangled\n").unwrap();

    let out = ralphy(&[
        "changes",
        "discard",
        "--repo",
        &root,
        "--path=a.txt",
        "--path=b.txt",
    ]);
    assert!(
        out.status.success(),
        "changes discard must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "one\n",
        "the tracked path is back to its committed content"
    );
    assert!(
        !repo.path().join("b.txt").exists(),
        "the untracked path is deleted"
    );

    let refused = ralphy(&["changes", "discard", "--repo", &root, "--path=a.txt"]);
    assert!(
        !refused.status.success(),
        "a clean path is no longer in the change set"
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("has no change —"),
        "the refusal is the core's prose: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

/// A staged deletion is IN the change set and still not restorable — measured,
/// `git restore --worktree -- f.txt` there exits 1 with `pathspec 'f.txt' did
/// not match any file(s) known to git`. The operator must read this module's
/// prose, never that string.
#[test]
fn changes_discard_refuses_a_staged_deletion_with_prose() {
    let repo = init_repo();
    run_git(repo.path(), &["rm", "--quiet", "a.txt"]);
    let before = git_state(repo.path());

    let out = ralphy(&[
        "changes",
        "discard",
        "--repo",
        &repo.path().to_string_lossy(),
        "--path=a.txt",
    ]);

    assert!(!out.status.success(), "a staged deletion must refuse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("has no working-tree change"),
        "the refusal reads as prose: {stderr}"
    );
    assert!(
        !stderr.contains("did not match any file"),
        "no git error string is relayed: {stderr}"
    );
    assert_eq!(git_state(repo.path()), before, "a refusal moves nothing");
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
        String::from_utf8_lossy(&refused.stderr).contains("has no change to stage —"),
        "the refusal is the core's prose: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

/// The held-lock rows of the four working-tree write verbs. Each row's state is
/// the index, `HEAD` and `a.txt`'s bytes: `discard` writes the WORKING TREE, so
/// it can move a file while leaving both the index and `HEAD` byte-identical.
pub(super) fn lock_rows() -> Vec<LockRow> {
    let stage = init_repo();

    let unstage = init_repo();
    run_git(unstage.path(), &["add", "b.txt"]);
    assert_eq!(
        git_state(unstage.path()).0,
        "b.txt",
        "the fixture really has a staged path"
    );

    let committed = init_repo();
    run_git(committed.path(), &["add", "b.txt"]);

    let discard = init_repo();
    std::fs::write(
        discard.path().join("a.txt"),
        "mangled
",
    )
    .unwrap();

    vec![
        lock_row("changes stage", stage, "--path=b.txt"),
        lock_row("changes unstage", unstage, "--path=b.txt"),
        lock_row("changes commit", committed, "--message=would land"),
        lock_row("changes discard", discard, "--path=a.txt"),
    ]
}

/// `<verb> --repo <repo> <flag>` against `repo`, which holds its own lock.
fn lock_row(verb: &'static str, repo: TempDir, flag: &str) -> LockRow {
    let root = repo.path().to_path_buf();
    let mut args: Vec<String> = verb.split(' ').map(String::from).collect();
    args.extend([
        "--repo".to_string(),
        root.to_string_lossy().to_string(),
        flag.to_string(),
    ]);
    let state_root = root.clone();
    LockRow {
        verb,
        _dirs: vec![repo],
        lock_repo: root,
        args,
        state: Box::new(move || {
            let (index, head) = git_state(&state_root);
            let a_txt = std::fs::read(state_root.join("a.txt")).unwrap();
            format!("index={index:?} head={head} a.txt={a_txt:?}")
        }),
    }
}
