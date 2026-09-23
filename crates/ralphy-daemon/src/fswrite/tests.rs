use super::*;
use std::fs;

#[test]
fn protected_dirs_refused_on_every_op() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    fs::write(root.path().join(".git/HEAD"), "ref: refs/heads/main").unwrap();
    // A recursive delete of `.git` is unrecoverable — the reason this exists.
    assert_eq!(delete(root.path(), ".git"), Err(WriteError::Confined));
    assert!(root.path().join(".git/HEAD").exists());
    // Nested, and through every other op.
    assert_eq!(
        write(root.path(), ".git/hooks/pre-commit", "#!/bin/sh"),
        Err(WriteError::Confined)
    );
    assert_eq!(
        create(root.path(), ".ralphy", true),
        Err(WriteError::Confined)
    );
    assert_eq!(
        write(root.path(), ".ralphy/settings.json", "{}"),
        Err(WriteError::Confined)
    );
    // Case-insensitively: NTFS resolves `.GIT` to the same directory.
    assert_eq!(
        write(root.path(), ".GIT/config", "x"),
        Err(WriteError::Confined)
    );
    // Refused as a rename DESTINATION as well as a source.
    write(root.path(), "note.txt", "hi").unwrap();
    assert_eq!(
        rename(root.path(), "note.txt", ".ralphy/note.txt"),
        Err(WriteError::Confined)
    );
    assert_eq!(
        copy(root.path(), "note.txt", ".ralphy/note.txt"),
        Err(WriteError::Confined)
    );
    // …and as a copy SOURCE: `.git` is not a place bytes are harvested from.
    assert_eq!(
        copy(root.path(), ".git/HEAD", "head.txt"),
        Err(WriteError::Confined)
    );
    assert!(!root.path().join("head.txt").exists());
    // Traversal out of the root is refused on the copy destination too.
    assert_eq!(
        copy(root.path(), "note.txt", "../x"),
        Err(WriteError::Confined)
    );
    assert!(!root.path().parent().unwrap().join("x").exists());
    // A name that merely CONTAINS a protected name stays writable.
    write(root.path(), ".gitignore", "target/").unwrap();
    write(root.path(), "gitlab.yml", "x").unwrap();
}

/// The Win32 name equivalences the lexical compare did not see (security
/// audit 2026-09-21, F1): a trailing dot or space is stripped by the Win32
/// layer, so `.git.` opens `.git`. The string check is platform-independent
/// even though only Windows rewrites the name — refused everywhere.
#[test]
fn protected_dirs_refused_under_trailing_dot_and_space_spellings() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    fs::write(root.path().join(".git/HEAD"), "ref: refs/heads/main").unwrap();
    fs::create_dir(root.path().join(".git/hooks")).unwrap();
    fs::create_dir(root.path().join("sub")).unwrap();
    write(root.path(), "note.txt", "hi").unwrap();

    for spelling in [
        ".git.",
        ".git ",
        ".gIt.",
        ".git..",
        ".ralphy.",
        ".ralphy ",
        ".git./hooks/pre-commit",
        ".git /hooks/pre-commit",
        "sub/.GIT./x",
    ] {
        assert_eq!(
            write(root.path(), spelling, "#!/bin/sh"),
            Err(WriteError::Confined),
            "write {spelling:?}"
        );
        assert_eq!(
            create(root.path(), spelling, false),
            Err(WriteError::Confined),
            "create file {spelling:?}"
        );
        assert_eq!(
            create(root.path(), spelling, true),
            Err(WriteError::Confined),
            "create dir {spelling:?}"
        );
        assert_eq!(
            delete(root.path(), spelling),
            Err(WriteError::Confined),
            "delete {spelling:?}"
        );
        assert_eq!(
            rename(root.path(), "note.txt", spelling),
            Err(WriteError::Confined),
            "rename into {spelling:?}"
        );
        assert_eq!(
            copy(root.path(), "note.txt", spelling),
            Err(WriteError::Confined),
            "copy into {spelling:?}"
        );
    }
    assert!(root.path().join(".git/HEAD").exists(), ".git survives");
    assert!(!root.path().join(".git/hooks/pre-commit").exists());
    assert!(root.path().join("note.txt").exists());

    // An NTFS alternate data stream spelling is refused on every platform —
    // the read side refuses `:` too, and the two must not drift.
    for ads in ["pwned.txt:hidden", ".git:x", "sub/a.txt:ads"] {
        assert_eq!(
            write(root.path(), ads, "x"),
            Err(WriteError::Confined),
            "ads {ads:?}"
        );
        assert_eq!(
            create(root.path(), ads, false),
            Err(WriteError::Confined),
            "ads {ads:?}"
        );
    }
    assert!(!root.path().join("pwned.txt").exists());

    // Names that merely resemble the denylist stay writable — including a
    // `~N` that is not a short name of anything.
    write(root.path(), "notes~1.txt", "x").unwrap();
    // A trailing dot on a NON-protected name is the OS's business (Windows
    // may refuse to create it), never the denylist's.
    assert_ne!(
        write(root.path(), ".gitignore.", "x"),
        Err(WriteError::Confined)
    );
}

/// The one hole, and its walls (ADR-0064 §5). A note in the landing
/// directory is reachable by every byte-op — that is what makes the
/// explorer's rename and delete work on a note — and NOTHING else inside
/// `.ralphy` moves an inch, including the spellings Windows rewrites.
#[test]
fn the_notes_carve_out_opens_exactly_one_shape() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join(".ralphy/notes")).unwrap();
    fs::write(root.path().join(".ralphy/settings.json"), "{}").unwrap();

    // Allowed: write, then rename, copy and delete the same note.
    write(root.path(), ".ralphy/notes/a.note", "bytes").unwrap();
    create(root.path(), ".ralphy/notes/b.note", false).unwrap();
    copy(root.path(), ".ralphy/notes/a.note", ".ralphy/notes/c.note").unwrap();
    rename(
        root.path(),
        ".ralphy/notes/a.note",
        ".ralphy/notes/renamed.note",
    )
    .unwrap();
    delete(root.path(), ".ralphy/notes/renamed.note").unwrap();
    // And moving one OUT of the landing dir, into the operator's tree.
    rename(root.path(), ".ralphy/notes/b.note", "kept.note").unwrap();
    assert!(root.path().join("kept.note").exists());

    // Refused: everything that is not exactly `.ralphy/notes/<name>.note`.
    for spelling in [
        ".ralphy/notes/x.md",          // not a note
        ".ralphy/notes/.note",         // no name
        ".ralphy/x.note",              // not in the landing dir
        ".ralphy/notes/sub/x.note",    // not directly in it
        ".ralphy/notes/x.note.",       // a Win32 rewrite of the name
        ".RALPHY/notes/x.note",        // the carve-out does not case-fold
        ".ralphy./notes/x.note",       // nor trim
        ".ralphy/NOTES/x.note",        // nor rename the landing dir
        ".git/notes/x.note",           // the other protected dir
        ".ralphy/notes/x.note:stream", // an alternate data stream
    ] {
        assert_eq!(
            write(root.path(), spelling, "x"),
            Err(WriteError::Confined),
            "write {spelling:?}"
        );
        assert_eq!(
            delete(root.path(), spelling),
            Err(WriteError::Confined),
            "delete {spelling:?}"
        );
        assert_eq!(
            rename(root.path(), ".ralphy/notes/c.note", spelling),
            Err(WriteError::Confined),
            "rename into {spelling:?}"
        );
    }
    // A rename may not carry a note into the rest of `.ralphy` either.
    assert_eq!(
        rename(
            root.path(),
            ".ralphy/notes/c.note",
            ".ralphy/worktrees/c.note"
        ),
        Err(WriteError::Confined)
    );
    assert_eq!(
        fs::read_to_string(root.path().join(".ralphy/settings.json")).unwrap(),
        "{}"
    );
    assert!(root.path().join(".ralphy/notes/c.note").exists());
}

/// A symlink AT the note file is refused before the denylist is reached:
/// `confine_write` lstats the final component and answers `Escape` for a
/// link (confine.rs), so the carve-out never gets to judge it. Named for
/// the guard it exercises — the RESOLVED-component gate has its own test
/// below, with a directory symlink that bypasses this one.
#[cfg(unix)]
#[test]
fn a_note_name_that_is_a_symlink_is_refused_before_the_denylist() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join(".ralphy/notes")).unwrap();
    fs::write(root.path().join(".ralphy/settings.json"), "{}").unwrap();
    symlink(
        root.path().join(".ralphy/settings.json"),
        root.path().join(".ralphy/notes/sneak.note"),
    )
    .unwrap();
    assert_eq!(
        write(root.path(), ".ralphy/notes/sneak.note", "pwned"),
        Err(WriteError::Confined)
    );
    assert_eq!(
        delete(root.path(), ".ralphy/notes/sneak.note"),
        Err(WriteError::Confined)
    );
    assert_eq!(
        fs::read_to_string(root.path().join(".ralphy/settings.json")).unwrap(),
        "{}"
    );
}

/// The RESOLVED gate, on its own. This is the case the second check exists
/// for and the one the spelled gate cannot see: `.ralphy/notes` is itself a
/// directory symlink at `.ralphy`, so the client's spelling
/// `.ralphy/notes/x.note` passes the carve-out lexically, `confine_write`
/// canonicalizes the PARENT to `<root>/.ralphy`, and only the component
/// check on the answer can refuse the write into daemon state.
///
/// The sibling test above exercises a symlink at the FILE, which
/// `confine_write` rejects by `symlink_metadata` before this gate is
/// reached — a different guard, and why both are here.
#[cfg(unix)]
#[test]
fn a_notes_dir_symlinked_at_daemon_state_is_refused_by_the_resolved_gate() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".ralphy")).unwrap();
    fs::write(root.path().join(".ralphy/settings.json"), "{}").unwrap();
    // `.ralphy/notes` -> `.ralphy`
    symlink(
        root.path().join(".ralphy"),
        root.path().join(".ralphy/notes"),
    )
    .unwrap();

    assert_eq!(
        write(root.path(), ".ralphy/notes/settings.json.note", "pwned"),
        Err(WriteError::Confined)
    );
    assert_eq!(
        write(root.path(), ".ralphy/notes/x.note", "pwned"),
        Err(WriteError::Confined)
    );
    assert_eq!(
        delete(root.path(), ".ralphy/notes/settings.json.note"),
        Err(WriteError::Confined)
    );
    assert_eq!(
        fs::read_to_string(root.path().join(".ralphy/settings.json")).unwrap(),
        "{}"
    );
    assert!(!root.path().join(".ralphy/x.note").exists());
}

/// And the same shape aimed OUT of the repo: the landing dir is a link to
/// somewhere else entirely, which `confine_write`'s canonical-parent check
/// refuses before the denylist is consulted at all.
#[cfg(unix)]
#[test]
fn a_notes_dir_symlinked_out_of_the_root_is_refused() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".ralphy")).unwrap();
    symlink(outside.path(), root.path().join(".ralphy/notes")).unwrap();

    assert_eq!(
        write(root.path(), ".ralphy/notes/x.note", "pwned"),
        Err(WriteError::Confined)
    );
    assert!(
        fs::read_dir(outside.path()).unwrap().next().is_none(),
        "nothing was written through the link"
    );
}

/// The resolved-path gate on its own, without Windows: an in-root symlink
/// aimed at `.git` never NAMES `.git`, so the lexical compare passes and
/// only canonicalizing the parent (which `confine_write` already does) and
/// re-checking the denylist on the answer can refuse it. `#[cfg(unix)]` for
/// the same reason as every other symlink test here.
#[cfg(unix)]
#[test]
fn protected_dir_reached_through_an_in_root_symlink_is_refused() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join(".git/hooks")).unwrap();
    symlink(root.path().join(".git"), root.path().join("link")).unwrap();

    assert_eq!(
        write(root.path(), "link/hooks/pre-commit", "#!/bin/sh"),
        Err(WriteError::Confined)
    );
    assert_eq!(
        create(root.path(), "link/hooks/d", true),
        Err(WriteError::Confined)
    );
    assert_eq!(delete(root.path(), "link/hooks"), Err(WriteError::Confined));
    assert!(root.path().join(".git/hooks").is_dir());
    assert!(!root.path().join(".git/hooks/pre-commit").exists());
}

/// The 8.3 short name — the one spelling the lexical gate deliberately
/// does not model. NTFS resolves `GIT~1` to `.git` at the filesystem
/// level, so only the resolved-path gate can refuse it. Short-name
/// generation is a per-volume setting (`fsutil 8dot3name`); when the temp
/// volume has it off there is nothing to test, and the test says so.
#[cfg(windows)]
#[test]
fn protected_dir_reached_through_a_short_name_is_refused() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join(".git/hooks")).unwrap();
    fs::write(root.path().join(".git/HEAD"), "ref: refs/heads/main").unwrap();
    if !root.path().join("GIT~1").exists() {
        println!("8.3 short names are off on this volume — nothing to refuse");
        return;
    }
    assert_eq!(
        write(root.path(), "GIT~1/hooks/pre-commit", "#!/bin/sh"),
        Err(WriteError::Confined)
    );
    assert_eq!(
        create(root.path(), "GIT~1/hooks/d", true),
        Err(WriteError::Confined)
    );
    assert_eq!(delete(root.path(), "GIT~1"), Err(WriteError::Confined));
    assert_eq!(
        delete(root.path(), "GIT~1/hooks"),
        Err(WriteError::Confined)
    );
    assert!(root.path().join(".git/HEAD").exists(), ".git survives");
    assert!(!root.path().join(".git/hooks/pre-commit").exists());
}

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
/// (tests/workspace_write.rs): making a symlink on Windows needs privileges CI
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
