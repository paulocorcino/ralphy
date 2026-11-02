use super::*;

#[test]
fn discard_plan_removes_only_the_plan_and_leaves_the_denylist_standing() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".ralphy")).unwrap();
    let plan = root.path().join(".ralphy").join("plan.md");
    let keep = root.path().join(".ralphy").join("settings.json");
    fs::write(&plan, "# Plan for #350\n<!-- ralphy-plan: issue=350 -->\n").unwrap();
    fs::write(&keep, "{}").unwrap();

    discard_plan(root.path()).unwrap();
    assert!(!plan.exists(), "the plan is gone");
    // Nothing ELSE in `.ralphy` is touched — this is not a clean-out of the
    // run's state directory, it is one artifact the operator owns.
    assert!(keep.exists(), "the rest of .ralphy survives");
    assert!(root.path().join(".ralphy").is_dir());

    // Absent is NotFound, not a silent ok: "there was no plan" and "the plan
    // is gone" are different answers, and the panel says which.
    assert_eq!(discard_plan(root.path()), Err(WriteError::NotFound));

    // The generic ops still refuse the SAME file. The narrow verb exists so
    // the denylist does not have to gain a hole; if this pair ever disagrees,
    // the hole is what happened.
    fs::write(&plan, "x").unwrap();
    assert_eq!(
        delete(root.path(), ".ralphy/plan.md"),
        Err(WriteError::Confined)
    );
    assert!(plan.exists());
    assert_eq!(
        write(root.path(), ".ralphy/plan.md", "rewritten"),
        Err(WriteError::Confined)
    );
}

#[test]
fn discard_plan_refuses_anything_that_is_not_a_regular_file() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join(".ralphy").join("plan.md")).unwrap();
    fs::write(root.path().join(".ralphy/plan.md/inner.txt"), "x").unwrap();
    // A DIRECTORY at that name is refused rather than recursively removed:
    // this path takes no client input, so a recursive delete is the one thing
    // it must never grow.
    assert_eq!(discard_plan(root.path()), Err(WriteError::Confined));
    assert!(root.path().join(".ralphy/plan.md/inner.txt").exists());
}

/// The escape this path could have had: `plan.md` as a symlink OUT of the
/// repo. `symlink_metadata` is what refuses it, and a `remove_file` through
/// the link would have unlinked the operator's own file elsewhere.
/// `#[cfg(unix)]` for the same reason as `symlink_write_escape_refused`
/// (tests/observe/workspace_write.rs): making a symlink on Windows needs privileges CI
/// does not have.
#[cfg(unix)]
#[test]
fn discard_plan_refuses_a_symlinked_plan_and_leaves_its_target() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("real-plan.md");
    fs::write(&target, "someone else's file").unwrap();
    fs::create_dir(root.path().join(".ralphy")).unwrap();
    symlink(&target, root.path().join(".ralphy").join("plan.md")).unwrap();

    assert_eq!(discard_plan(root.path()), Err(WriteError::Confined));
    assert!(target.exists(), "the symlink's target is untouched");
}

#[test]
fn write_creates_and_overwrites() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "note.txt", "hi").unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("note.txt")).unwrap(),
        "hi"
    );
    write(root.path(), "note.txt", "bye").unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("note.txt")).unwrap(),
        "bye"
    );
}

#[test]
fn create_folder_then_conflict() {
    let root = tempfile::tempdir().unwrap();
    create(root.path(), "newdir", true).unwrap();
    assert!(root.path().join("newdir").is_dir());
    assert_eq!(
        create(root.path(), "newdir", true),
        Err(WriteError::Conflict)
    );
    create(root.path(), "f.txt", false).unwrap();
    assert_eq!(
        create(root.path(), "f.txt", false),
        Err(WriteError::Conflict)
    );
}

#[test]
fn rename_moves_and_refuses_existing_dst() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "a.txt", "x").unwrap();
    rename(root.path(), "a.txt", "b.txt").unwrap();
    assert!(!root.path().join("a.txt").exists());
    assert!(root.path().join("b.txt").exists());
    // Renaming an absent source is NotFound.
    assert_eq!(
        rename(root.path(), "a.txt", "c.txt"),
        Err(WriteError::NotFound)
    );
    // Renaming onto an existing dst is Conflict.
    write(root.path(), "d.txt", "y").unwrap();
    assert_eq!(
        rename(root.path(), "b.txt", "d.txt"),
        Err(WriteError::Conflict)
    );
}

/// `rename` is the byte-op behind the explorer's MOVE, so its destination is
/// now operator-chosen rather than a sibling name — the denylist and
/// confinement are what stand between a picked directory and `.git`/`.ralphy`
/// or the world outside the root. Negative control, one line per assert:
/// swapping `confine_outside_protected(root, to_rel)` for a bare
/// `confine_write` fails assert 1 and doing the same for `from_rel` fails
/// assert 2 — each line is killed by exactly one of them, since the other
/// still catches the pair.
#[test]
fn rename_refuses_protected_dirs_and_escape() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "a.txt", "x").unwrap();
    // `create` refuses `.ralphy`, so the fixture is laid down with `std::fs`.
    fs::create_dir(root.path().join(".ralphy")).unwrap();
    fs::write(root.path().join(".ralphy/keep.txt"), "keep").unwrap();

    // Into a protected dir…
    assert_eq!(
        rename(root.path(), "a.txt", ".ralphy/a.txt"),
        Err(WriteError::Confined)
    );
    // …and OUT of one.
    assert_eq!(
        rename(root.path(), ".ralphy/keep.txt", "a2.txt"),
        Err(WriteError::Confined)
    );
    // Out of the root entirely.
    assert_eq!(
        rename(root.path(), "a.txt", "../escaped.txt"),
        Err(WriteError::Confined)
    );

    assert!(root.path().join("a.txt").exists(), "the source survives");
    assert!(!root.path().join(".ralphy/a.txt").exists());
    assert!(root.path().join(".ralphy/keep.txt").exists());
    assert!(!root.path().join("a2.txt").exists());
    assert!(!root.path().parent().unwrap().join("escaped.txt").exists());
}

#[test]
fn copy_duplicates_bytes_and_refuses_existing_dst() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "a.txt", "x").unwrap();
    copy(root.path(), "a.txt", "a copy.txt").unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("a copy.txt")).unwrap(),
        "x"
    );
    // The source SURVIVES — the invariant that separates copy from rename.
    assert_eq!(fs::read_to_string(root.path().join("a.txt")).unwrap(), "x");
    // Copying onto an existing dst is Conflict, never an overwrite.
    assert_eq!(
        copy(root.path(), "a.txt", "a copy.txt"),
        Err(WriteError::Conflict)
    );
    assert_eq!(
        copy(root.path(), "gone.txt", "b.txt"),
        Err(WriteError::NotFound)
    );
    // A directory source is refused rather than copied recursively.
    create(root.path(), "d", true).unwrap();
    assert_eq!(copy(root.path(), "d", "d2"), Err(WriteError::Confined));
    assert!(!root.path().join("d2").exists());
}

/// The claim `copy`'s doc makes about `symlink_metadata` — that it refuses the
/// LINK rather than following it. Without a case here, swapping it for
/// `metadata` breaks nothing: confinement catches an out-of-root target
/// incidentally, so the link is aimed at an IN-root file, where only the
/// `is_file()` check on the link itself can refuse it. `#[cfg(unix)]` for the
/// same reason as the other symlink tests: making one on Windows needs
/// privileges CI does not have.
#[cfg(unix)]
#[test]
fn copy_refuses_a_symlinked_source_even_inside_the_root() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "real.txt", "secret").unwrap();
    symlink(root.path().join("real.txt"), root.path().join("link.txt")).unwrap();
    assert_eq!(
        copy(root.path(), "link.txt", "leak.txt"),
        Err(WriteError::Confined)
    );
    assert!(!root.path().join("leak.txt").exists());
}

#[test]
fn delete_removes_file_and_dir() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "f.txt", "x").unwrap();
    delete(root.path(), "f.txt").unwrap();
    assert!(!root.path().join("f.txt").exists());
    // A populated dir deletes recursively.
    create(root.path(), "d", true).unwrap();
    write(root.path(), "d/inner.txt", "y").unwrap();
    delete(root.path(), "d").unwrap();
    assert!(!root.path().join("d").exists());
    assert_eq!(delete(root.path(), "gone"), Err(WriteError::NotFound));
}

#[test]
fn write_refuses_traversal() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(write(root.path(), "../x", "hi"), Err(WriteError::Confined));
    assert!(!root.path().parent().unwrap().join("x").exists());
}

#[test]
fn write_error_keeps_its_text() {
    let rows: [(WriteError, &str); 5] = [
        (WriteError::Confined, "path escapes the repo root"),
        (WriteError::Conflict, "path already exists"),
        (WriteError::NotFound, "path not found"),
        (
            WriteError::Unencodable { char_index: 3 },
            "character 3 is not representable in the target encoding",
        ),
        (WriteError::Io, "io error"),
    ];
    for (err, text) in rows {
        assert_eq!(err.to_string(), text);
    }
}
