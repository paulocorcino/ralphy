use super::*;
use crate::git::git;
use crate::scratch::remove_scratch;
use std::path::PathBuf;

fn init_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ralphy-changes-{}-{}", std::process::id(), name));
    remove_scratch(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]).unwrap();
    git(&dir, &["config", "user.email", "t@example.com"]).unwrap();
    git(&dir, &["config", "user.name", "Test"]).unwrap();
    dir
}

fn find<'a>(list: &'a [Change], path: &str) -> &'a Change {
    list.iter()
        .find(|c| c.path == path)
        .unwrap_or_else(|| panic!("no entry for {path}, got: {list:?}"))
}

#[test]
fn modified_added_deleted_renamed_untracked() {
    let dir = init_repo("all-statuses");
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    std::fs::write(dir.join("old.txt"), "old\n").unwrap();
    std::fs::write(dir.join("gone.txt"), "gone\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(&dir, &["commit", "-q", "-m", "init"]).unwrap();

    std::fs::write(dir.join("README.md"), "changed\n").unwrap();
    std::fs::write(dir.join("added.txt"), "new\n").unwrap();
    git(&dir, &["add", "added.txt"]).unwrap();
    std::fs::remove_file(dir.join("gone.txt")).unwrap();
    git(&dir, &["mv", "old.txt", "new.txt"]).unwrap();
    std::fs::write(dir.join("untracked.txt"), "loose\n").unwrap();

    let list = changes(&dir).unwrap();

    assert_eq!(find(&list, "README.md").status, ChangeStatus::Modified);
    assert_eq!(find(&list, "README.md").original_path, None);
    assert_eq!(find(&list, "added.txt").status, ChangeStatus::Added);
    assert_eq!(find(&list, "gone.txt").status, ChangeStatus::Deleted);
    assert_eq!(
        *find(&list, "new.txt"),
        Change {
            path: "new.txt".to_string(),
            original_path: Some("old.txt".to_string()),
            status: ChangeStatus::Renamed,
            index_status: Some(ChangeStatus::Renamed),
            worktree_status: None,
        }
    );
    assert_eq!(find(&list, "untracked.txt").status, ChangeStatus::Untracked);
    assert_eq!(
        list.len(),
        5,
        "exactly one entry per changed path: {list:?}"
    );

    remove_scratch(&dir);
}

#[test]
fn path_with_space_and_non_ascii_survive() {
    let dir = init_repo("odd-paths");
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(&dir, &["commit", "-q", "-m", "init"]).unwrap();

    std::fs::write(dir.join("a file.txt"), "spaced\n").unwrap();
    std::fs::write(dir.join("café.txt"), "accented\n").unwrap();
    git(&dir, &["add", "."]).unwrap();

    let list = changes(&dir).unwrap();
    let paths: Vec<&str> = list.iter().map(|c| c.path.as_str()).collect();
    assert!(paths.contains(&"a file.txt"), "spaced path: {paths:?}");
    assert!(paths.contains(&"café.txt"), "non-ASCII path: {paths:?}");
    assert_eq!(list.len(), 2, "exactly the two seeded paths: {list:?}");

    remove_scratch(&dir);
}

#[test]
fn clean_tree_is_empty() {
    let dir = init_repo("clean");
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(&dir, &["commit", "-q", "-m", "init"]).unwrap();

    assert_eq!(changes(&dir).unwrap(), vec![]);

    remove_scratch(&dir);
}

#[test]
fn run_artifacts_are_excluded() {
    // The `.gitignore` deliberately does NOT list `.ralphy/`: the filter is
    // the change set's own, not git's.
    let dir = init_repo("run-artifacts");
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(&dir, &["commit", "-q", "-m", "init"]).unwrap();

    std::fs::create_dir_all(dir.join(".ralphy")).unwrap();
    std::fs::write(dir.join(".ralphy").join("plan.md"), "scratch\n").unwrap();
    std::fs::write(dir.join("README.md"), "changed\n").unwrap();

    let list = changes(&dir).unwrap();
    assert_eq!(list.len(), 1, "only the tracked edit: {list:?}");
    assert_eq!(list[0].path, "README.md");

    remove_scratch(&dir);
}

#[test]
fn run_directory_only_change_is_clean_for_core_and_git_read() {
    let dir = init_repo("agreement");
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(&dir, &["commit", "-q", "-m", "init"]).unwrap();

    std::fs::create_dir_all(dir.join(".ralphy")).unwrap();
    std::fs::write(dir.join(".ralphy").join("x.txt"), "scratch\n").unwrap();
    assert_eq!(changes(&dir).unwrap(), Vec::<Change>::new());
    assert!(!ralphy_git_read::dirty(&dir).unwrap());

    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    assert_ne!(changes(&dir).unwrap(), Vec::<Change>::new());
    assert!(ralphy_git_read::dirty(&dir).unwrap());

    remove_scratch(&dir);
}

#[test]
fn the_run_artifact_filter_is_anchored_at_the_repo_root() {
    // The behaviour this reader deliberately changed: the old rule matched
    // `.ralphy/` ANYWHERE in the porcelain line, so a nested run directory
    // escaped too — and `runner::branch` refuses a run on a non-clean tree,
    // so that miss silently let a dirty repo through.
    let dir = init_repo("nested-ralphy");
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(&dir, &["commit", "-q", "-m", "init"]).unwrap();

    std::fs::create_dir_all(dir.join(".ralphy")).unwrap();
    std::fs::write(dir.join(".ralphy").join("plan.md"), "scratch\n").unwrap();
    std::fs::create_dir_all(dir.join("sub").join(".ralphy")).unwrap();
    std::fs::write(dir.join("sub").join(".ralphy").join("real.md"), "real\n").unwrap();

    let list = changes(&dir).unwrap();
    let paths: Vec<&str> = list.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(list.len(), 1, "only the nested path is a change: {list:?}");
    assert!(
        paths[0].starts_with("sub/"),
        "a nested `.ralphy/` is real work, not run scratch: {paths:?}"
    );
    assert!(
        !crate::git::is_clean_ignoring_ralphy(&dir).unwrap(),
        "a repo dirty under a NESTED .ralphy/ is not clean"
    );

    remove_scratch(&dir);
}

/// The regression guard for the index split (#315): the derived `status`
/// must read exactly as it did before the two side fields existed. `AM` is
/// the live negative control — reading the WORKTREE side instead of the
/// first non-`.` char turns it into `Modified`, and nothing else here would
/// notice. `RM` is NOT a control for `ordinary_status` (the `2` arm returns
/// a literal `Renamed` and never calls it); it is asserted here because the
/// two readings must AGREE, which the `2` arm's own check below pins.
#[test]
fn the_derived_status_is_unchanged_by_the_index_split() {
    let dir = init_repo("derived-projection");
    // `old.txt` is long enough that the post-rename append stays well above
    // git's 50% rename-similarity threshold, so the record is a `2`.
    std::fs::write(dir.join("old.txt"), "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n").unwrap();
    std::fs::write(dir.join("edited.txt"), "hello\n").unwrap();
    std::fs::write(dir.join("gone.txt"), "gone\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(&dir, &["commit", "-q", "-m", "init"]).unwrap();

    std::fs::write(dir.join("edited.txt"), "changed\n").unwrap();
    std::fs::write(dir.join("added.txt"), "new\n").unwrap();
    git(&dir, &["add", "added.txt"]).unwrap();
    std::fs::write(dir.join("staged-then-edited.txt"), "staged\n").unwrap();
    git(&dir, &["add", "staged-then-edited.txt"]).unwrap();
    std::fs::write(dir.join("staged-then-edited.txt"), "staged\nmore\n").unwrap();
    git(&dir, &["rm", "-q", "gone.txt"]).unwrap();
    git(&dir, &["mv", "old.txt", "new.txt"]).unwrap();
    std::fs::write(dir.join("new.txt"), "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\n").unwrap();
    std::fs::write(dir.join("untracked.txt"), "loose\n").unwrap();

    let list = changes(&dir).unwrap();
    for (path, expected) in [
        ("edited.txt", ChangeStatus::Modified),
        ("added.txt", ChangeStatus::Added),
        ("staged-then-edited.txt", ChangeStatus::Added),
        ("gone.txt", ChangeStatus::Deleted),
        ("new.txt", ChangeStatus::Renamed),
        ("untracked.txt", ChangeStatus::Untracked),
    ] {
        assert_eq!(
            find(&list, path).status,
            expected,
            "the derived status of {path} must not move: {list:?}"
        );
    }
    // The two divergence cases really are two-sided — without this the
    // assertions above would also hold for a repo git reported as one-sided.
    assert_eq!(
        find(&list, "staged-then-edited.txt").worktree_status,
        Some(ChangeStatus::Modified),
        "`AM`: the staged add was edited again on disk"
    );
    assert_eq!(
        find(&list, "new.txt").worktree_status,
        Some(ChangeStatus::Modified),
        "`RM`: the rename was edited again on disk"
    );
    // The `2` arm's literal `Renamed` and the first-non-`.` reading must
    // AGREE — `index_status` IS that first side, so this is the comparison
    // the literal would otherwise let drift unnoticed.
    let renamed = find(&list, "new.txt");
    assert_eq!(
        renamed.index_status,
        Some(renamed.status),
        "the rename arm's literal status must equal its first non-`.` side"
    );

    remove_scratch(&dir);
}

#[test]
fn staged_then_modified_splits_across_both_sides() {
    let dir = init_repo("both-sides");
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(&dir, &["commit", "-q", "-m", "init"]).unwrap();

    std::fs::write(dir.join("both.txt"), "staged\n").unwrap();
    git(&dir, &["add", "both.txt"]).unwrap();
    std::fs::write(dir.join("both.txt"), "staged\nand edited\n").unwrap();

    let list = changes(&dir).unwrap();
    let entry = find(&list, "both.txt");
    assert_eq!(entry.index_status, Some(ChangeStatus::Added));
    assert_eq!(entry.worktree_status, Some(ChangeStatus::Modified));
    assert_eq!(
        entry.status,
        ChangeStatus::Added,
        "the projection still answers what differs from HEAD"
    );
    assert_eq!(
        list.len(),
        1,
        "one entry per path, not one per side: {list:?}"
    );

    remove_scratch(&dir);
}

#[test]
fn a_staged_delete_and_a_rename_split_by_side() {
    let dir = init_repo("split-by-side");
    std::fs::write(dir.join("old.txt"), "l1\nl2\nl3\nl4\n").unwrap();
    std::fs::write(dir.join("gone.txt"), "gone\n").unwrap();
    std::fs::write(dir.join("edited.txt"), "hello\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(&dir, &["commit", "-q", "-m", "init"]).unwrap();

    git(&dir, &["rm", "-q", "gone.txt"]).unwrap();
    std::fs::write(dir.join("added.txt"), "new\n").unwrap();
    git(&dir, &["add", "added.txt"]).unwrap();
    git(&dir, &["mv", "old.txt", "new.txt"]).unwrap();
    std::fs::write(dir.join("edited.txt"), "changed\n").unwrap();

    let list = changes(&dir).unwrap();
    for (path, index, worktree) in [
        ("gone.txt", Some(ChangeStatus::Deleted), None),
        ("added.txt", Some(ChangeStatus::Added), None),
        ("new.txt", Some(ChangeStatus::Renamed), None),
        ("edited.txt", None, Some(ChangeStatus::Modified)),
    ] {
        let entry = find(&list, path);
        assert_eq!(entry.index_status, index, "index side of {path}");
        assert_eq!(entry.worktree_status, worktree, "worktree side of {path}");
    }
    assert_eq!(
        find(&list, "new.txt").original_path.as_deref(),
        Some("old.txt")
    );

    remove_scratch(&dir);
}

#[test]
fn an_untracked_and_a_conflicted_entry_have_no_index_side() {
    let dir = init_repo("no-index-side");
    std::fs::write(dir.join("README.md"), "base\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(&dir, &["commit", "-q", "-m", "init"]).unwrap();

    git(&dir, &["checkout", "-q", "-b", "other"]).unwrap();
    std::fs::write(dir.join("README.md"), "theirs\n").unwrap();
    git(&dir, &["commit", "-q", "-am", "theirs"]).unwrap();
    git(&dir, &["checkout", "-q", "main"]).unwrap();
    std::fs::write(dir.join("README.md"), "ours\n").unwrap();
    git(&dir, &["commit", "-q", "-am", "ours"]).unwrap();
    // The merge FAILS by design — `git()` bails on a non-zero exit.
    let merged = std::process::Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["merge", "other"])
        .output()
        .unwrap();
    assert!(!merged.status.success(), "the merge must conflict");
    std::fs::write(dir.join("untracked.txt"), "loose\n").unwrap();

    let list = changes(&dir).unwrap();
    let conflicted = find(&list, "README.md");
    assert_eq!(conflicted.index_status, None, "a conflict is not staged");
    assert_eq!(
        conflicted.worktree_status,
        Some(ChangeStatus::Conflicted),
        "an unresolved conflict is worktree work"
    );
    assert_eq!(conflicted.status, ChangeStatus::Conflicted);

    let untracked = find(&list, "untracked.txt");
    assert_eq!(untracked.index_status, None);
    assert_eq!(untracked.worktree_status, Some(ChangeStatus::Untracked));

    remove_scratch(&dir);
}

#[test]
fn a_merge_conflict_reads_as_conflicted() {
    let dir = init_repo("conflict");
    std::fs::write(dir.join("README.md"), "base\n").unwrap();
    git(&dir, &["add", "."]).unwrap();
    git(&dir, &["commit", "-q", "-m", "init"]).unwrap();

    git(&dir, &["checkout", "-q", "-b", "other"]).unwrap();
    std::fs::write(dir.join("README.md"), "theirs\n").unwrap();
    git(&dir, &["commit", "-q", "-am", "theirs"]).unwrap();
    git(&dir, &["checkout", "-q", "main"]).unwrap();
    std::fs::write(dir.join("README.md"), "ours\n").unwrap();
    git(&dir, &["commit", "-q", "-am", "ours"]).unwrap();
    // The merge FAILS by design — `git()` bails on a non-zero exit, so the
    // conflict is provoked through the raw command.
    let merged = std::process::Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["merge", "other"])
        .output()
        .unwrap();
    assert!(!merged.status.success(), "the merge must conflict");

    let list = changes(&dir).unwrap();
    assert_eq!(
        list,
        vec![Change {
            path: "README.md".to_string(),
            original_path: None,
            status: ChangeStatus::Conflicted,
            index_status: None,
            worktree_status: Some(ChangeStatus::Conflicted),
        }],
        "an unmerged path reads as Conflicted"
    );

    remove_scratch(&dir);
}

#[test]
fn is_clean_ignoring_ralphy_agrees_with_the_reader() {
    let seed = |dir: &PathBuf| {
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        git(dir, &["add", "."]).unwrap();
        git(dir, &["commit", "-q", "-m", "init"]).unwrap();
    };

    let clean = init_repo("clean-agree");
    seed(&clean);

    let scratch = init_repo("scratch-agree");
    seed(&scratch);
    std::fs::create_dir_all(scratch.join(".ralphy")).unwrap();
    std::fs::write(scratch.join(".ralphy").join("plan.md"), "scratch\n").unwrap();

    let dirty = init_repo("dirty-agree");
    seed(&dirty);
    std::fs::write(dirty.join("README.md"), "changed\n").unwrap();

    for (dir, expected) in [(&clean, true), (&scratch, true), (&dirty, false)] {
        let clean_check = crate::git::is_clean_ignoring_ralphy(dir).unwrap();
        assert_eq!(clean_check, expected, "clean check on {}", dir.display());
        assert_eq!(
            clean_check,
            changes(dir).unwrap().is_empty(),
            "the two definitions must agree on {}",
            dir.display()
        );
        remove_scratch(dir);
    }
}
