//! The working-tree change set: one reader over `git status --porcelain=v2 -z`
//! that answers "what differs from HEAD" for a repo. This is the single
//! definition of a dirty tree — `git::is_clean_ignoring_ralphy` is expressed on
//! top of it.

use std::path::Path;

use anyhow::Result;
use serde::Serialize;

/// What happened to a path, as the change set reports it. Answers "what differs
/// from HEAD" — the index/worktree split is not modelled.
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub path: String,
    pub original_path: Option<String>,
    pub status: ChangeStatus,
}

/// The repo's working-tree change set, in git's own emission order. Entries
/// under the gitignored run directory (`.ralphy/`) are never reported.
pub fn changes(repo: &Path) -> Result<Vec<Change>> {
    let out = crate::git::git(repo, &["status", "--porcelain=v2", "-z"])?;
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
            }),
            Some(b'1') => field(record, 9).map(|path| Change {
                path,
                original_path: None,
                status: ordinary_status(record),
            }),
            Some(b'2') => {
                let original = records.get(i).map(|t| t.to_string());
                i += 1;
                field(record, 10).map(|path| Change {
                    path,
                    original_path: original,
                    status: ChangeStatus::Renamed,
                })
            }
            Some(b'u') => field(record, 11).map(|path| Change {
                path,
                original_path: None,
                status: ChangeStatus::Conflicted,
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

/// Status of an ordinary (`1 `) record: the first non-`.` char of its two-char
/// `XY` field. The surface answers "what differs from HEAD", so a staged add
/// that was then edited is one `Added`, not two states.
fn ordinary_status(record: &str) -> ChangeStatus {
    let xy = record.split(' ').nth(1).unwrap_or("");
    match xy.chars().find(|c| *c != '.') {
        Some('A') => ChangeStatus::Added,
        Some('D') => ChangeStatus::Deleted,
        Some('R') | Some('C') => ChangeStatus::Renamed,
        Some('U') => ChangeStatus::Conflicted,
        _ => ChangeStatus::Modified,
    }
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
