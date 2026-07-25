//! The working-tree change set: one reader over `git status --porcelain=v2 -z`
//! that answers "what differs from HEAD" for a repo. This is the single
//! definition of a dirty tree — `git::is_clean_ignoring_ralphy` is expressed on
//! top of it.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

/// What happened to a path, as the change set reports it. Answers "what differs
/// from HEAD" — the index/worktree split is not modelled, so a path staged as
/// added and then deleted from disk still reports `Added`: the index side wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

/// One changed path. `original_path` is set only for a rename/copy.
///
/// `index_status` and `worktree_status` are the two sides of git's `XY` field,
/// `None` meaning unmodified on that side (git's `.`). `status` is the derived
/// projection over both — see [`ChangeStatus`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub path: String,
    pub original_path: Option<String>,
    pub status: ChangeStatus,
    pub index_status: Option<ChangeStatus>,
    pub worktree_status: Option<ChangeStatus>,
}

/// The repo's working-tree change set, in git's own emission order. Entries
/// under the gitignored run directory (`.ralphy/`) are never reported.
///
/// `repo` must be the git TOPLEVEL: git reports the whole repo's change set from
/// any directory inside it, while the run-artifact filter anchors at the root, so
/// a subdirectory path would yield root-relative entries under a mismatched
/// filter. Callers resolve with [`crate::git::resolve_toplevel`].
pub fn changes(repo: &Path) -> Result<Vec<Change>> {
    let out = crate::git::git(repo, &["status", "--porcelain=v2", "-z"])
        .with_context(|| format!("reading the change set of {}", repo.display()))?;
    let records: Vec<&str> = out.split('\0').filter(|t| !t.is_empty()).collect();

    let mut list = Vec::new();
    // Explicit index: a `2 ` (rename/copy) record consumes the NEXT NUL-framed
    // token as its original path.
    let mut i = 0;
    while i < records.len() {
        let record = records[i];
        i += 1;
        let entry = match record.as_bytes().first() {
            Some(b'?') => field(record, 2).map(|path| Change {
                path,
                original_path: None,
                status: ChangeStatus::Untracked,
                index_status: None,
                worktree_status: Some(ChangeStatus::Untracked),
            }),
            Some(b'1') => field(record, 9).map(|path| {
                let (index_status, worktree_status) = sides(record);
                Change {
                    path,
                    original_path: None,
                    status: ordinary_status(record),
                    index_status,
                    worktree_status,
                }
            }),
            Some(b'2') => {
                let original = records.get(i).map(|t| t.to_string());
                i += 1;
                field(record, 10).map(|path| {
                    let (index_status, worktree_status) = sides(record);
                    Change {
                        path,
                        original_path: original,
                        status: ChangeStatus::Renamed,
                        index_status,
                        worktree_status,
                    }
                })
            }
            // An unresolved conflict is worktree work whatever its XY reads: a
            // per-char split would file `AA` under the index side and claim a
            // commit would contain it.
            Some(b'u') => field(record, 11).map(|path| Change {
                path,
                original_path: None,
                status: ChangeStatus::Conflicted,
                index_status: None,
                worktree_status: Some(ChangeStatus::Conflicted),
            }),
            // `!` (ignored) and `#` (headers) carry no change.
            _ => None,
        };
        if let Some(entry) = entry {
            if is_run_artifact(&entry.path)
                || entry.original_path.as_deref().is_some_and(is_run_artifact)
            {
                continue;
            }
            list.push(entry);
        }
    }
    Ok(list)
}

/// The path field of a porcelain-v2 record: everything after the first
/// `count - 1` space-separated fields. Only the path itself may contain a
/// space, so a bounded split is exact.
fn field(record: &str, count: usize) -> Option<String> {
    record
        .splitn(count, ' ')
        .nth(count - 1)
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
}

/// The two-char `XY` status field of a `1`/`2` record.
fn xy(record: &str) -> &str {
    record.split(' ').nth(1).unwrap_or("")
}

/// One side of `XY`: `.` is unmodified on that side, and every other char falls
/// back to `Modified` (`M`, `T`, and anything a future git adds).
fn side_status(c: char) -> Option<ChangeStatus> {
    Some(match c {
        '.' => return None,
        'A' => ChangeStatus::Added,
        'D' => ChangeStatus::Deleted,
        'R' | 'C' => ChangeStatus::Renamed,
        'U' => ChangeStatus::Conflicted,
        _ => ChangeStatus::Modified,
    })
}

/// Both sides of a `1`/`2` record, index first. A short or absent `XY` reads as
/// no change on the missing side rather than a guess.
fn sides(record: &str) -> (Option<ChangeStatus>, Option<ChangeStatus>) {
    let mut chars = xy(record).chars();
    let index = chars.next().and_then(side_status);
    let worktree = chars.next().and_then(side_status);
    (index, worktree)
}

/// Status of an ordinary (`1 `) record: the first non-`.` char of its two-char
/// `XY` field. The surface answers "what differs from HEAD", so a staged add
/// that was then edited is one `Added`, not two states. Expressed over
/// [`side_status`] so the projection and the split can never drift apart.
fn ordinary_status(record: &str) -> ChangeStatus {
    xy(record)
        .chars()
        .find(|c| *c != '.')
        .and_then(side_status)
        .unwrap_or(ChangeStatus::Modified)
}

/// Anchored at the repo root: a nested `docs/.ralphy/x` is a real change, only
/// the run directory itself is scratch.
fn is_run_artifact(path: &str) -> bool {
    path.starts_with(".ralphy/") || path.starts_with(".ralphy\\")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::git;
    use std::path::PathBuf;

    fn init_repo(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ralphy-changes-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
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

        let _ = std::fs::remove_dir_all(&dir);
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

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_tree_is_empty() {
        let dir = init_repo("clean");
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        git(&dir, &["add", "."]).unwrap();
        git(&dir, &["commit", "-q", "-m", "init"]).unwrap();

        assert_eq!(changes(&dir).unwrap(), vec![]);

        let _ = std::fs::remove_dir_all(&dir);
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

        let _ = std::fs::remove_dir_all(&dir);
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

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The regression guard for the index split (#315): the derived `status`
    /// must read exactly as it did before the two side fields existed. `AM` and
    /// `RM` are the negative control — reading the WORKTREE side instead of the
    /// first non-`.` char turns both into `Modified`, and nothing else here
    /// would notice.
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

        let _ = std::fs::remove_dir_all(&dir);
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

        let _ = std::fs::remove_dir_all(&dir);
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

        let _ = std::fs::remove_dir_all(&dir);
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

        let _ = std::fs::remove_dir_all(&dir);
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

        let _ = std::fs::remove_dir_all(&dir);
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
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
