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
    // This host leaves LF in the blob and CRLF on disk, which would make an
    // exact-content oracle over a restored file a coin flip.
    git(dir, &["config", "core.autocrlf", "false"]).unwrap();
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

/// The regression test for the group-stage abort. REPRODUCED live before the
/// fix: `git --literal-pathspecs add -- renamed.txt old.txt other.txt` exits
/// 128 with `fatal: pathspec 'old.txt' did not match any files` AND stages
/// nothing but `renamed.txt` — `git add` aborts the WHOLE invocation on one
/// unmatched pathspec. So a single rename-then-edit entry made the panel's
/// group `+` stage nothing, relaying git's raw prose on the way out.
#[test]
fn stage_refuses_a_renames_original_path_and_stages_nothing_partially() {
    let dir = init_repo("rename-stage");
    commit_file(&dir, "old.txt", "l1\nl2\nl3\nl4\nl5\nl6\n", "add old");
    std::fs::write(dir.join("other.txt"), "loose\n").unwrap();
    git(&dir, &["mv", "old.txt", "renamed.txt"]).unwrap();
    // …then edit it, so the entry is `RM` and lands in BOTH groups.
    std::fs::write(dir.join("renamed.txt"), "l1\nl2\nl3\nl4\nl5\nl6\nl7\n").unwrap();
    let before = staged_paths(&dir);

    let refused = stage(
        &dir,
        &[
            "renamed.txt".to_string(),
            "old.txt".to_string(),
            "other.txt".to_string(),
        ],
    )
    .unwrap();
    assert_eq!(
        refused,
        StageOutcome::NotInChangeSet {
            path: "old.txt".to_string()
        },
        "the rename's original path is not stageable"
    );
    assert_eq!(
        refused.reason().as_deref(),
        Some("cannot stage: old.txt is not in the change set"),
        "the operator reads this module's prose, never git's `fatal:`"
    );
    assert_eq!(
        staged_paths(&dir),
        before,
        "a refusal stages NOTHING — not even the good half of the list"
    );

    // The positive control: without the old path the same request succeeds, so
    // the refusal above is about that path and not about the fixture.
    assert_eq!(
        stage(&dir, &["renamed.txt".to_string(), "other.txt".to_string()]).unwrap(),
        StageOutcome::Staged { paths: 2 }
    );
    // …and `unstage` still accepts the original path, which is the direction
    // that genuinely needs it.
    assert_eq!(
        unstage(&dir, &["renamed.txt".to_string(), "old.txt".to_string()]).unwrap(),
        UnstageOutcome::Unstaged { paths: 2 }
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_restores_a_tracked_file_from_head() {
    let dir = init_repo("discard-tracked");
    std::fs::write(dir.join("never-touched.txt"), "mangled\n").unwrap();

    assert_eq!(
        discard(&dir, &p("never-touched.txt")).unwrap(),
        DiscardOutcome::Discarded {
            restored: 1,
            deleted: 0
        }
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("never-touched.txt")).unwrap(),
        "steady\n",
        "the committed content is back on disk"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_deletes_an_untracked_file() {
    let dir = init_repo("discard-untracked");
    std::fs::write(dir.join("loose.txt"), "never committed\n").unwrap();

    // The counts are the classifier's oracle: an inverted tracked/untracked
    // partition flips them even though the file would still leave disk.
    assert_eq!(
        discard(&dir, &p("loose.txt")).unwrap(),
        DiscardOutcome::Discarded {
            restored: 0,
            deleted: 1
        }
    );
    assert!(!dir.join("loose.txt").exists(), "the file is gone");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_deletes_an_untracked_directory() {
    // `git status --porcelain=v2` reports an untracked DIRECTORY as ONE entry
    // named `newdir/` — so that entry shape is what the panel offers, and
    // `clean -d` is what makes it discardable.
    let dir = init_repo("discard-untracked-dir");
    std::fs::create_dir_all(dir.join("newdir/sub")).unwrap();
    std::fs::write(dir.join("newdir/sub/f.txt"), "deep\n").unwrap();

    assert!(
        changes(&dir).unwrap().iter().any(|c| c.path == "newdir/"),
        "git names the whole directory as one entry, got: {:?}",
        changes(&dir).unwrap()
    );
    assert_eq!(
        discard(&dir, &p("newdir/")).unwrap(),
        DiscardOutcome::Discarded {
            restored: 0,
            deleted: 1
        }
    );
    assert!(!dir.join("newdir").exists(), "the directory is gone");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_leaves_every_other_path_alone() {
    let dir = init_repo("discard-isolation");
    commit_file(&dir, "a.txt", "one\n", "add a");
    commit_file(&dir, "b.txt", "two\n", "add b");
    std::fs::write(dir.join("a.txt"), "edited a\n").unwrap();
    std::fs::write(dir.join("b.txt"), "edited b\n").unwrap();
    std::fs::write(dir.join("loose.txt"), "loose\n").unwrap();

    assert_eq!(
        discard(&dir, &p("a.txt")).unwrap(),
        DiscardOutcome::Discarded {
            restored: 1,
            deleted: 0
        }
    );
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\n");
    assert_eq!(
        std::fs::read_to_string(dir.join("b.txt")).unwrap(),
        "edited b\n",
        "the other modified path keeps its edit"
    );
    assert!(
        dir.join("loose.txt").exists(),
        "an untracked bystander is not cleaned"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The negative control for "a staged change is never silently thrown away".
/// This is the test that reds if the implementation ever reaches for
/// `--source=HEAD --staged`.
#[test]
fn discard_keeps_a_staged_change() {
    let dir = init_repo("discard-staged");
    commit_file(&dir, "both.txt", "base\n", "add both");
    std::fs::write(dir.join("both.txt"), "staged\n").unwrap();
    git(&dir, &["add", "both.txt"]).unwrap();
    std::fs::write(dir.join("both.txt"), "worktree only\n").unwrap();

    let mixed = entry(&dir, "both.txt");
    assert!(
        mixed.index_status.is_some() && mixed.worktree_status.is_some(),
        "the fixture must be staged AND edited again, got {mixed:?}"
    );

    assert_eq!(
        discard(&dir, &p("both.txt")).unwrap(),
        DiscardOutcome::Discarded {
            restored: 1,
            deleted: 0
        }
    );
    assert_eq!(
        git(&dir, &["show", ":both.txt"]).unwrap(),
        "staged",
        "the staged blob survives an unstaged discard"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("both.txt")).unwrap(),
        "staged\n",
        "the working tree went back to the INDEX, not to HEAD"
    );
    assert_eq!(staged_paths(&dir), "both.txt", "the path is still staged");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_refuses_a_path_outside_the_change_set() {
    let dir = init_repo("discard-unknown");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();

    let refused = discard(&dir, &p("never-touched.txt")).unwrap();
    assert_eq!(
        refused,
        DiscardOutcome::NotInChangeSet {
            path: "never-touched.txt".to_string()
        }
    );
    assert_eq!(
        refused.reason().as_deref(),
        Some("cannot discard: never-touched.txt is not in the change set")
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("never-touched.txt")).unwrap(),
        "steady\n",
        "a refusal ran no git write"
    );
    assert!(dir.join("a.txt").exists(), "and cleaned nothing either");
    assert_eq!(staged_paths(&dir), "");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn discard_refuses_an_empty_list() {
    let dir = init_repo("discard-empty");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();

    assert_eq!(discard(&dir, &[]).unwrap(), DiscardOutcome::NoPaths);
    assert!(dir.join("a.txt").exists(), "nothing on disk moved");
    assert_eq!(
        std::fs::read_to_string(dir.join("never-touched.txt")).unwrap(),
        "steady\n"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `--literal-pathspecs` is what keeps a filename from being read as a pattern
/// at the LAST hop. Without the flag `git add "a[0].txt"` treats the name as a
/// glob, matches nothing (there is no `a0.txt`), and exits non-zero — so
/// deleting the flag reds this test. `[` is legal on both platforms, unlike `*`,
/// which Windows forbids in a filename.
#[test]
fn a_bracketed_filename_is_staged_literally_not_globbed() {
    let dir = init_repo("literal-pathspecs");
    std::fs::write(dir.join("a[0].txt"), "bracketed\n").unwrap();

    assert_eq!(
        stage(&dir, &p("a[0].txt")).unwrap(),
        StageOutcome::Staged { paths: 1 }
    );
    assert_eq!(
        entry(&dir, "a[0].txt").index_status,
        Some(ChangeStatus::Added),
        "the literal name reached the index"
    );
    assert_eq!(
        unstage(&dir, &p("a[0].txt")).unwrap(),
        UnstageOutcome::Unstaged { paths: 1 }
    );

    let _ = std::fs::remove_dir_all(&dir);
}
