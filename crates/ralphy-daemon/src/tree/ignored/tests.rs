use std::fs;
use std::path::Path;

use crate::tree::list;

/// `(name, ignored)` for every entry `list` returns at `rel`.
fn marks(root: &Path, rel: &str) -> Vec<(String, bool)> {
    list(root, rel)
        .unwrap()
        .into_iter()
        .map(|e| (e.name, e.ignored))
        .collect()
}

fn ignored(marks: &[(String, bool)], name: &str) -> bool {
    marks
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("{name} not listed: {marks:?}"))
        .1
}

#[test]
fn nothing_is_ignored_without_rules() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.txt"), b"x").unwrap();
    fs::create_dir(root.path().join("dir")).unwrap();
    let m = marks(root.path(), "");
    assert!(!ignored(&m, "a.txt") && !ignored(&m, "dir"), "{m:?}");
}

#[test]
fn root_gitignore_marks_files_and_dirs_but_still_lists_them() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join(".gitignore"), b"build/\n*.tmp\n").unwrap();
    fs::create_dir(root.path().join("build")).unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("a.tmp"), b"x").unwrap();
    fs::write(root.path().join("a.txt"), b"x").unwrap();
    let m = marks(root.path(), "");
    assert!(ignored(&m, "build"), "{m:?}");
    assert!(ignored(&m, "a.tmp"), "{m:?}");
    assert!(!ignored(&m, "src"), "{m:?}");
    assert!(!ignored(&m, "a.txt"), "{m:?}");
    assert!(!ignored(&m, ".gitignore"), "{m:?}");
}

#[test]
fn everything_inside_an_ignored_dir_is_ignored() {
    // Git never looks inside an ignored directory, so a nested whitelist
    // cannot bring a file back.
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join(".gitignore"), b"build/\n").unwrap();
    fs::create_dir_all(root.path().join("build/deep")).unwrap();
    fs::write(root.path().join("build/.gitignore"), b"!keep.txt\n").unwrap();
    fs::write(root.path().join("build/keep.txt"), b"x").unwrap();
    let m = marks(root.path(), "build");
    assert!(ignored(&m, "keep.txt"), "{m:?}");
    assert!(ignored(&m, "deep"), "{m:?}");
    assert!(ignored(&m, ".gitignore"), "{m:?}");
}

#[test]
fn a_nested_gitignore_applies_only_below_its_dir() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("sub")).unwrap();
    // `/gen` is anchored to `sub/`, so it must be matched as `gen`, not as
    // `sub/gen`.
    fs::write(root.path().join("sub/.gitignore"), b"local.txt\n/gen\n").unwrap();
    fs::write(root.path().join("sub/local.txt"), b"x").unwrap();
    fs::create_dir(root.path().join("sub/gen")).unwrap();
    fs::write(root.path().join("local.txt"), b"x").unwrap();
    let sub = marks(root.path(), "sub");
    assert!(ignored(&sub, "local.txt"), "{sub:?}");
    assert!(ignored(&sub, "gen"), "{sub:?}");
    assert!(!ignored(&marks(root.path(), ""), "local.txt"));
}

#[test]
fn a_negated_pattern_whitelists() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join(".gitignore"), b"*.out\n!keep.out\n").unwrap();
    fs::write(root.path().join("drop.out"), b"x").unwrap();
    fs::write(root.path().join("keep.out"), b"x").unwrap();
    let m = marks(root.path(), "");
    assert!(ignored(&m, "drop.out"), "{m:?}");
    assert!(!ignored(&m, "keep.out"), "{m:?}");
}

#[test]
fn an_anchored_pattern_matches_only_at_its_own_dir() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join(".gitignore"), b"/out\n").unwrap();
    fs::create_dir_all(root.path().join("sub/out")).unwrap();
    fs::create_dir(root.path().join("out")).unwrap();
    assert!(ignored(&marks(root.path(), ""), "out"));
    assert!(!ignored(&marks(root.path(), "sub"), "out"));
}

#[test]
fn info_exclude_is_honoured() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join(".git/info")).unwrap();
    fs::write(root.path().join(".git/info/exclude"), b"secret.txt\n").unwrap();
    fs::write(root.path().join("secret.txt"), b"x").unwrap();
    assert!(ignored(&marks(root.path(), ""), "secret.txt"));
}

#[test]
fn a_linked_worktree_reads_the_shared_info_exclude() {
    // A worktree's `.git` is a file; its git dir's `commondir` leads back to
    // the primary's `.git`, whose `info/exclude` every worktree shares.
    let primary = tempfile::tempdir().unwrap();
    let git = primary.path().join(".git");
    fs::create_dir_all(git.join("info")).unwrap();
    fs::write(git.join("info/exclude"), b"shared.txt\n").unwrap();
    let wt_git = git.join("worktrees").join("wt");
    fs::create_dir_all(&wt_git).unwrap();
    fs::write(wt_git.join("commondir"), b"../..\n").unwrap();
    let wt = tempfile::tempdir().unwrap();
    fs::write(
        wt.path().join(".git"),
        format!("gitdir: {}\n", wt_git.display()),
    )
    .unwrap();
    fs::write(wt.path().join("shared.txt"), b"x").unwrap();
    fs::write(wt.path().join("mine.txt"), b"x").unwrap();
    let m = marks(wt.path(), "");
    assert!(ignored(&m, "shared.txt"), "{m:?}");
    assert!(!ignored(&m, "mine.txt"), "{m:?}");
}
