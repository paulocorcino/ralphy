//! Every fixture here is a temporary repository built by `git init` — nothing in
//! this suite names a remote, so none of it can touch the network.

use super::*;
use crate::changes::ChangeStatus;
use crate::git::git;
use crate::scratch::remove_scratch;
use std::path::PathBuf;

mod discard;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ralphy-worktree-{}-{}", std::process::id(), name));
    remove_scratch(&dir);
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
    vec![s.to_owned()]
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

    remove_scratch(&dir);
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
            path: "never-touched.txt".to_owned()
        }
    );
    assert_eq!(
        refused.reason().as_deref(),
        Some(
            "cannot stage: never-touched.txt has no change to stage — \
             it may be staged, committed or reverted already"
        )
    );
    assert_eq!(
        staged_paths(&dir),
        "",
        "a refusal must leave the index untouched"
    );

    remove_scratch(&dir);
}

#[test]
fn stage_refuses_an_empty_list() {
    let dir = init_repo("stage-empty");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();

    assert_eq!(stage(&dir, &[]).unwrap(), StageOutcome::NoPaths);
    assert_eq!(staged_paths(&dir), "");

    remove_scratch(&dir);
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

    remove_scratch(&dir);
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

    remove_scratch(&dir);
}

#[test]
fn unstage_refuses_a_path_outside_the_change_set() {
    let dir = init_repo("unstage-unknown");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();

    let refused = unstage(&dir, &p("never-touched.txt")).unwrap();
    assert_eq!(
        refused,
        UnstageOutcome::NotInChangeSet {
            path: "never-touched.txt".to_owned()
        }
    );
    assert_eq!(
        refused.reason().as_deref(),
        Some(
            "cannot unstage: never-touched.txt has no change — \
             it may be committed or reverted already"
        )
    );
    assert_eq!(unstage(&dir, &[]).unwrap(), UnstageOutcome::NoPaths);

    remove_scratch(&dir);
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
        unstage(&dir, &["new.txt".to_owned(), "old.txt".to_owned()]).unwrap(),
        UnstageOutcome::Unstaged { paths: 2 }
    );
    assert_eq!(
        staged_paths(&dir),
        "",
        "both halves of the rename left the index"
    );

    remove_scratch(&dir);
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
    let failure = refused.failure().expect("a refusal carries a failure");
    assert_eq!(failure.domain.as_str(), "working-tree");
    assert_eq!(failure.code, "nothing-staged");
    assert_eq!(commit_count(&dir), before, "no commit was created");

    remove_scratch(&dir);
}

#[test]
fn commit_with_an_empty_message_is_refused() {
    let dir = init_repo("empty-message");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();
    stage(&dir, &p("a.txt")).unwrap();
    let before = commit_count(&dir);

    let refused = commit(&dir, "   ").unwrap();
    assert_eq!(refused, CommitOutcome::EmptyMessage);
    let failure = refused.failure().expect("a refusal carries a failure");
    assert_eq!(failure.domain.as_str(), "working-tree");
    assert_eq!(failure.code, "empty-commit-message");
    assert_eq!(commit_count(&dir), before, "no commit was created");
    assert_eq!(
        staged_paths(&dir),
        "a.txt",
        "the refusal left the staged index alone"
    );

    remove_scratch(&dir);
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
    assert_eq!(done.failure(), None, "a success carries no failure");
    assert_eq!(commit_count(&dir).parse::<usize>().unwrap(), before + 1);
    assert_eq!(changes(&dir).unwrap(), vec![], "the tree is clean again");
    assert_eq!(
        git(&dir, &["log", "-1", "--format=%s"]).unwrap(),
        "feat: a thing"
    );

    remove_scratch(&dir);
}

#[test]
fn a_commit_git_refuses_carries_the_line_that_says_why() {
    let dir = init_repo("commit-refused");
    // A signer that does not exist: git refuses with no script of ours.
    git(&dir, &["config", "commit.gpgsign", "true"]).unwrap();
    git(&dir, &["config", "gpg.program", "ralphy-no-such-signer"]).unwrap();
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();
    stage(&dir, &p("a.txt")).unwrap();

    let err = commit(&dir, "feat: a thing").unwrap_err();
    let failure = err
        .chain()
        .find_map(|e| e.downcast_ref::<ralphy_wording::Failure>())
        .unwrap_or_else(|| panic!("no failure in the chain: {err:#}"));
    assert_eq!(failure.code, "commit-refused");
    assert!(
        failure.text.cause.contains("ralphy-no-such-signer"),
        "{failure:?}"
    );
    assert!(!failure.text.cause.contains('\n'), "one line: {failure:?}");
    assert!(
        format!("{err:#}").contains("failed to write commit object"),
        "{err:#}"
    );

    remove_scratch(&dir);
}

#[test]
fn a_commit_message_beginning_with_a_dash_is_recorded_verbatim() {
    // MEASURED: git itself accepts `-m -oops` too, so this leg alone does not
    // discriminate the fusion — it pins that the message reaches git intact.
    // The hop the fusion really protects is clap's, covered end-to-end by
    // `crates/ralphy-cli/tests/verbs/worktree.rs`.
    let dir = init_repo("dash-message");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();
    stage(&dir, &p("a.txt")).unwrap();

    let done = commit(&dir, "-oops").unwrap();
    assert!(
        matches!(done, CommitOutcome::Committed { .. }),
        "got {done:?}"
    );
    assert_eq!(git(&dir, &["log", "-1", "--format=%s"]).unwrap(), "-oops");

    remove_scratch(&dir);
}

/// The group-stage abort: `git add -- renamed.txt old.txt other.txt` exits 128
/// on the rename's old path and stages nothing, so `stage` must never pass it.
#[test]
fn stage_skips_a_renames_original_path_and_stages_the_rest() {
    let dir = init_repo("rename-stage");
    commit_file(&dir, "old.txt", "l1\nl2\nl3\nl4\nl5\nl6\n", "add old");
    std::fs::write(dir.join("other.txt"), "loose\n").unwrap();
    git(&dir, &["mv", "old.txt", "renamed.txt"]).unwrap();
    // …then edit it, so the entry is `RM` and lands in BOTH groups.
    std::fs::write(dir.join("renamed.txt"), "l1\nl2\nl3\nl4\nl5\nl6\nl7\n").unwrap();

    let list = [
        "renamed.txt".to_owned(),
        "old.txt".to_owned(),
        "other.txt".to_owned(),
    ];
    assert_eq!(
        stage(&dir, &list).unwrap(),
        StageOutcome::Staged { paths: 2 }
    );
    let staged = staged_paths(&dir);
    assert!(staged.contains("other.txt"), "the rest is staged: {staged}");
    assert!(changes(&dir)
        .unwrap()
        .iter()
        .all(|c| c.worktree_status.is_none()));
    // The old path alone has nothing to stage.
    assert_eq!(
        stage(&dir, &p("old.txt")).unwrap(),
        StageOutcome::NotInChangeSet {
            path: "old.txt".to_owned()
        }
    );
    // `unstage` accepts the original path, the direction that needs it.
    assert_eq!(
        unstage(&dir, &["renamed.txt".to_owned(), "old.txt".to_owned()]).unwrap(),
        UnstageOutcome::Unstaged { paths: 2 }
    );

    remove_scratch(&dir);
}

/// A list read before a file was reverted: the reverted path is skipped.
#[test]
fn unstage_skips_a_path_that_left_the_change_set() {
    let dir = init_repo("unstage-stale");
    std::fs::write(dir.join("a.txt"), "new\n").unwrap();
    stage(&dir, &p("a.txt")).unwrap();

    let list = ["a.txt".to_owned(), "never-touched.txt".to_owned()];
    assert_eq!(
        unstage(&dir, &list).unwrap(),
        UnstageOutcome::Unstaged { paths: 1 }
    );
    assert_eq!(staged_paths(&dir), "");

    remove_scratch(&dir);
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

    remove_scratch(&dir);
}
