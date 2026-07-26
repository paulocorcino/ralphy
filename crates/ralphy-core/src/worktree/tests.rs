//! Every fixture here is a temporary repository built by `git init` — nothing in
//! this suite names a remote, so none of it can touch the network.

use super::*;
use crate::changes::ChangeStatus;
use crate::git::git;
use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ralphy-worktree-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn configure(dir: &Path) {
    git(dir, &["config", "user.email", "t@example.com"]).unwrap();
    git(dir, &["config", "user.name", "Test"]).unwrap();
}

/// A repo on `main` with NO commit yet — the unborn-HEAD fixture.
fn init_unborn(name: &str) -> PathBuf {
    let dir = tmp(name);
    git(&dir, &["init", "-q", "-b", "main"]).unwrap();
    configure(&dir);
    dir
}

fn commit_file(dir: &Path, file: &str, body: &str, msg: &str) {
    std::fs::write(dir.join(file), body).unwrap();
    git(dir, &["add", "."]).unwrap();
    git(dir, &["commit", "-q", "-m", msg]).unwrap();
}

/// A repo with one commit holding `never-touched.txt`, clean.
fn init_repo(name: &str) -> PathBuf {
    let dir = init_unborn(name);
    commit_file(&dir, "never-touched.txt", "steady\n", "init");
    dir
}

fn entry(dir: &Path, path: &str) -> crate::changes::Change {
    let list = changes(dir).unwrap();
    list.iter()
        .find(|c| c.path == path)
        .unwrap_or_else(|| panic!("no change-set entry for {path}, got: {list:?}"))
        .clone()
}

fn staged_paths(dir: &Path) -> String {
    git(dir, &["diff", "--cached", "--name-only"]).unwrap()
}

fn commit_count(dir: &Path) -> String {
    git(dir, &["rev-list", "--count", "HEAD"]).unwrap()
}

fn p(s: &str) -> Vec<String> {
    vec![s.to_string()]
}

#[test]
fn stage_moves_a_path_to_the_index() {
    let dir = init_repo("stage-one");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();

    assert_eq!(
        stage(&dir, &p("a.txt")).unwrap(),
        StageOutcome::Staged { paths: 1 }
    );
    assert_eq!(
        entry(&dir, "a.txt").index_status,
        Some(ChangeStatus::Added),
        "the path must be on the index side after staging"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stage_refuses_a_path_outside_the_change_set() {
    // `never-touched.txt` EXISTS on disk and is committed unmodified, so git
    // itself would happily accept it — only the change-set rule can refuse it.
    let dir = init_repo("stage-unknown");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();

    let refused = stage(&dir, &p("never-touched.txt")).unwrap();
    assert_eq!(
        refused,
        StageOutcome::NotInChangeSet {
            path: "never-touched.txt".to_string()
        }
    );
    assert_eq!(
        refused.reason().as_deref(),
        Some("cannot stage: never-touched.txt is not in the change set")
    );
    assert_eq!(
        staged_paths(&dir),
        "",
        "a refusal must leave the index untouched"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stage_refuses_an_empty_list() {
    let dir = init_repo("stage-empty");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();

    assert_eq!(stage(&dir, &[]).unwrap(), StageOutcome::NoPaths);
    assert_eq!(staged_paths(&dir), "");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stage_then_unstage_round_trips() {
    let dir = init_repo("round-trip");
    commit_file(&dir, "a.txt", "one\n", "add a");
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();

    assert_eq!(
        stage(&dir, &p("a.txt")).unwrap(),
        StageOutcome::Staged { paths: 1 }
    );
    assert!(
        entry(&dir, "a.txt").index_status.is_some(),
        "staged: the entry has an index side"
    );

    assert_eq!(
        unstage(&dir, &p("a.txt")).unwrap(),
        UnstageOutcome::Unstaged { paths: 1 }
    );
    let back = entry(&dir, "a.txt");
    assert_eq!(back.index_status, None, "unstaged: no index side left");
    assert_eq!(
        back.worktree_status,
        Some(ChangeStatus::Modified),
        "the edit itself survives — unstage touches the index, not the file"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unstage_on_an_unborn_head_returns_the_file_to_untracked() {
    // The negative control for the unborn-HEAD branch: `git restore --staged`
    // cannot resolve HEAD here, so inverting the probe makes this test error.
    let dir = init_unborn("unborn");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();

    assert_eq!(
        stage(&dir, &p("a.txt")).unwrap(),
        StageOutcome::Staged { paths: 1 }
    );
    assert_eq!(
        unstage(&dir, &p("a.txt")).unwrap(),
        UnstageOutcome::Unstaged { paths: 1 }
    );
    assert_eq!(
        entry(&dir, "a.txt").status,
        ChangeStatus::Untracked,
        "a fresh repo's first file returns to untracked, not to a staged delete"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unstage_refuses_a_path_outside_the_change_set() {
    let dir = init_repo("unstage-unknown");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();

    let refused = unstage(&dir, &p("never-touched.txt")).unwrap();
    assert_eq!(
        refused,
        UnstageOutcome::NotInChangeSet {
            path: "never-touched.txt".to_string()
        }
    );
    assert_eq!(
        refused.reason().as_deref(),
        Some("cannot unstage: never-touched.txt is not in the change set")
    );
    assert_eq!(unstage(&dir, &[]).unwrap(), UnstageOutcome::NoPaths);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rename_is_unstageable_by_its_original_path_too() {
    // `git restore --staged` needs the OLD path to undo the deletion half; the
    // change set names it only as `original_path`, so a set built from `path`
    // alone would refuse it as unknown.
    let dir = init_repo("rename");
    commit_file(&dir, "old.txt", "l1\nl2\nl3\nl4\n", "add old");
    git(&dir, &["mv", "old.txt", "new.txt"]).unwrap();

    assert_eq!(
        unstage(&dir, &["new.txt".to_string(), "old.txt".to_string()]).unwrap(),
        UnstageOutcome::Unstaged { paths: 2 }
    );
    assert_eq!(
        staged_paths(&dir),
        "",
        "both halves of the rename left the index"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn commit_with_nothing_staged_is_refused_and_creates_no_commit() {
    let dir = init_repo("nothing-staged");
    // An UNSTAGED modification: the change set is non-empty, so only the index
    // side can settle this.
    std::fs::write(dir.join("never-touched.txt"), "edited\n").unwrap();
    let before = commit_count(&dir);

    let refused = commit(&dir, "a real message").unwrap();
    assert_eq!(refused, CommitOutcome::NothingStaged);
    assert_eq!(
        refused.reason().as_deref(),
        Some("cannot commit: nothing is staged — stage a file first")
    );
    assert_eq!(commit_count(&dir), before, "no commit was created");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn commit_with_an_empty_message_is_refused() {
    let dir = init_repo("empty-message");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();
    stage(&dir, &p("a.txt")).unwrap();
    let before = commit_count(&dir);

    let refused = commit(&dir, "   ").unwrap();
    assert_eq!(refused, CommitOutcome::EmptyMessage);
    assert_eq!(
        refused.reason().as_deref(),
        Some("cannot commit: the message is empty")
    );
    assert_eq!(commit_count(&dir), before, "no commit was created");
    assert_eq!(
        staged_paths(&dir),
        "a.txt",
        "the refusal left the staged index alone"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn commit_writes_the_staged_index() {
    let dir = init_repo("commit-ok");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();
    stage(&dir, &p("a.txt")).unwrap();
    let before: usize = commit_count(&dir).parse().unwrap();

    let done = commit(&dir, "feat: a thing").unwrap();
    let CommitOutcome::Committed { sha } = &done else {
        panic!("expected a commit, got {done:?}");
    };
    assert!(!sha.is_empty(), "the sha is reported back");
    assert_eq!(done.reason(), None, "a success carries no refusal prose");
    assert_eq!(commit_count(&dir).parse::<usize>().unwrap(), before + 1);
    assert_eq!(changes(&dir).unwrap(), vec![], "the tree is clean again");
    assert_eq!(
        git(&dir, &["log", "-1", "--format=%s"]).unwrap(),
        "feat: a thing"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_commit_message_beginning_with_a_dash_is_recorded_verbatim() {
    // MEASURED: git itself accepts `-m -oops` too, so this leg alone does not
    // discriminate the fusion — it pins that the message reaches git intact.
    // The hop the fusion really protects is clap's, covered end-to-end by
    // `crates/ralphy-cli/tests/worktree.rs`.
    let dir = init_repo("dash-message");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();
    stage(&dir, &p("a.txt")).unwrap();

    let done = commit(&dir, "-oops").unwrap();
    assert!(
        matches!(done, CommitOutcome::Committed { .. }),
        "got {done:?}"
    );
    assert_eq!(git(&dir, &["log", "-1", "--format=%s"]).unwrap(), "-oops");

    let _ = std::fs::remove_dir_all(&dir);
}
