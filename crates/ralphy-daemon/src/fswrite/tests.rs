use super::*;
use std::fs;

mod operations;

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
