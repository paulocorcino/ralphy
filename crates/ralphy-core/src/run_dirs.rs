//! Retention of the run folders under `<repo>/.ralphy/runs/`.
//!
//! Only a folder whose name is exactly a run stamp (`AAAAMMDD-HHMMSS`) is
//! counted or deleted. Any other name, such as a renamed folder, is how an
//! operator keeps a run forever.

use std::io;
use std::path::{Path, PathBuf};

/// What [`prune_run_dirs`] did.
#[derive(Debug, Default)]
pub struct Prune {
    /// The folders removed, or on a dry run the folders that would be removed.
    pub removed: Vec<PathBuf>,
    /// The folders that could not be removed, with the reason.
    pub failed: Vec<(PathBuf, io::Error)>,
}

/// True when `name` is exactly 8 digits, a hyphen, and 6 digits.
fn is_run_stamp(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 15
        && b[8] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 8 || c.is_ascii_digit())
}

/// Keep the newest `keep` stamp folders of `runs_dir` and remove the older
/// ones with `remove`. The order is by folder name, never by modification time.
/// `keep == 0` removes nothing, `current` is never removed, and a missing or
/// unreadable `runs_dir` is an empty result. A folder that `remove` refuses is
/// listed in `failed`; the others are still removed.
pub fn prune_run_dirs(
    runs_dir: &Path,
    keep: u64,
    current: &Path,
    dry_run: bool,
    remove: impl Fn(&Path) -> io::Result<()>,
) -> Prune {
    let mut out = Prune::default();
    if keep == 0 {
        return out;
    }
    let Ok(entries) = std::fs::read_dir(runs_dir) else {
        return out;
    };
    let mut stamps: Vec<(String, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            is_run_stamp(&name).then(|| (name, e.path()))
        })
        .collect();
    stamps.sort_by(|a, b| b.0.cmp(&a.0));
    let skip = usize::try_from(keep).unwrap_or(usize::MAX);
    for (_, path) in stamps.into_iter().skip(skip) {
        if path == current {
            continue;
        }
        if dry_run {
            out.removed.push(path);
            continue;
        }
        match remove(&path) {
            Ok(()) => out.removed.push(path),
            Err(e) => out.failed.push((path, e)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make(runs: &Path, names: &[&str]) {
        for n in names {
            fs::create_dir_all(runs.join(n)).unwrap();
        }
    }

    fn names_left(runs: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(runs)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }

    fn prune(runs: &Path, keep: u64, dry_run: bool) -> Prune {
        let current = runs.join("99999999-999999");
        prune_run_dirs(runs, keep, &current, dry_run, |p| fs::remove_dir_all(p))
    }

    #[test]
    fn the_oldest_of_51_folders_is_removed_when_keeping_50() {
        let tmp = tempfile::tempdir().unwrap();
        let names: Vec<String> = (0..51).map(|i| format!("20260101-{i:06}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        make(tmp.path(), &refs);
        let out = prune(tmp.path(), 50, false);
        assert_eq!(out.removed, vec![tmp.path().join("20260101-000000")]);
        let left = names_left(tmp.path());
        assert_eq!(left.len(), 50);
        assert!(!left.contains(&"20260101-000000".to_owned()));
    }

    #[test]
    fn a_renamed_folder_and_a_loose_file_survive() {
        let tmp = tempfile::tempdir().unwrap();
        make(
            tmp.path(),
            &[
                "20260101-000001",
                "20260101-000002",
                "20260101-000000-keep",
                "20260101-0000000",
            ],
        );
        fs::write(tmp.path().join("notes.txt"), "x").unwrap();
        fs::write(tmp.path().join("20260101-000000"), "a file").unwrap();
        let out = prune(tmp.path(), 1, false);
        assert!(out.failed.is_empty(), "{:?}", out.failed);
        assert_eq!(
            names_left(tmp.path()),
            vec![
                "20260101-000000",
                "20260101-000000-keep",
                "20260101-0000000",
                "20260101-000002",
                "notes.txt"
            ]
        );
    }

    #[test]
    fn keep_zero_removes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        make(tmp.path(), &["20260101-000001", "20260101-000002"]);
        let out = prune(tmp.path(), 0, false);
        assert!(out.removed.is_empty());
        assert_eq!(names_left(tmp.path()).len(), 2);
    }

    #[test]
    fn the_newest_names_stay_whatever_the_modification_order() {
        let tmp = tempfile::tempdir().unwrap();
        // The newest name is created first, so it has the oldest mtime.
        make(
            tmp.path(),
            &["20260103-000000", "20260101-000000", "20260102-000000"],
        );
        prune(tmp.path(), 2, false);
        assert_eq!(
            names_left(tmp.path()),
            vec!["20260102-000000", "20260103-000000"]
        );
    }

    #[test]
    fn a_dry_run_lists_the_folders_and_deletes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        make(tmp.path(), &["20260101-000001", "20260101-000002"]);
        let out = prune(tmp.path(), 1, true);
        assert_eq!(out.removed, vec![tmp.path().join("20260101-000001")]);
        assert_eq!(names_left(tmp.path()).len(), 2);
    }

    #[test]
    fn a_folder_that_cannot_be_removed_is_reported_and_the_others_go() {
        let tmp = tempfile::tempdir().unwrap();
        make(
            tmp.path(),
            &["20260101-000001", "20260101-000002", "20260101-000003"],
        );
        // `remove_dir` fails on a folder that holds a file, on every platform.
        fs::write(tmp.path().join("20260101-000002").join("f"), "x").unwrap();
        let current = tmp.path().join("99999999-999999");
        let out = prune_run_dirs(tmp.path(), 1, &current, false, |p| fs::remove_dir(p));
        assert_eq!(out.failed.len(), 1);
        assert_eq!(out.failed[0].0, tmp.path().join("20260101-000002"));
        assert_eq!(out.removed, vec![tmp.path().join("20260101-000001")]);
        assert_eq!(
            names_left(tmp.path()),
            vec!["20260101-000002", "20260101-000003"]
        );
    }

    #[test]
    fn the_current_folder_is_never_removed() {
        let tmp = tempfile::tempdir().unwrap();
        make(tmp.path(), &["20260101-000001", "20260101-000002"]);
        let current = tmp.path().join("20260101-000001");
        let out = prune_run_dirs(tmp.path(), 1, &current, false, |p| fs::remove_dir_all(p));
        assert!(out.removed.is_empty());
        assert_eq!(names_left(tmp.path()).len(), 2);
    }

    #[test]
    fn a_missing_runs_folder_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let out = prune(&tmp.path().join("none"), 5, false);
        assert!(out.removed.is_empty() && out.failed.is_empty());
    }
}
