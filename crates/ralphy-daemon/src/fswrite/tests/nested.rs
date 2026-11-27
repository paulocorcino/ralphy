use super::*;

#[test]
fn a_nested_folder_creates_every_missing_level() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("docs")).unwrap();
    assert_eq!(create(root.path(), "docs/.agents/skills", true), Ok(()));
    assert!(root.path().join("docs/.agents").is_dir());
    assert!(root.path().join("docs/.agents/skills").is_dir());
}

#[test]
fn a_nested_path_that_exists_is_a_conflict() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("a/b")).unwrap();
    assert_eq!(create(root.path(), "a/b", true), Err(WriteError::Conflict));
}

#[test]
fn a_malformed_nested_path_is_refused() {
    for (name, rel) in [
        ("a parent step", "a/../b"),
        ("an empty level", "a//b"),
        ("a backslash", "a\\b"),
        ("a dot level", "./a"),
        ("a trailing slash", "a/"),
    ] {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            create(root.path(), rel, true),
            Err(WriteError::Confined),
            "{name}"
        );
        let left = fs::read_dir(root.path()).unwrap().count();
        assert_eq!(left, 0, "{name}: nothing is created");
    }
}

#[test]
fn a_protected_dir_at_any_level_is_refused() {
    for rel in [".git/x", "a/.ralphy/b", "a/.GIT./b"] {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            create(root.path(), rel, true),
            Err(WriteError::Confined),
            "{rel}"
        );
        let left = fs::read_dir(root.path()).unwrap().count();
        assert_eq!(left, 0, "{rel}: no level is created");
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_level_out_of_the_root_is_refused() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), root.path().join("out")).unwrap();
    assert_eq!(
        create(root.path(), "out/x/y", true),
        Err(WriteError::Confined)
    );
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn a_nested_file_creates_its_parent_folders() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(create(root.path(), "a/b/c.txt", false), Ok(()));
    assert!(root.path().join("a/b").is_dir());
    assert_eq!(fs::read(root.path().join("a/b/c.txt")).unwrap(), b"");
}
