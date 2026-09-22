//! The Write byte-op path (ADR-0036 §2, Write effect class): five pure functions
//! over a confined target — [`write`], [`create`], [`rename`], [`copy`],
//! [`delete`]. Like
//! [`crate::tree`] on the read side, this module carries NO repo semantics and
//! does NOT consult the run lock (ADR-0036 amendment: "Write does not consult the
//! run lock" — operator-owns-the-tree). Confinement ([`crate::confine`]) is the
//! security boundary in SPACE: every op resolves its target through
//! [`confine::confine_write`], which confines a maybe-missing target by confining
//! its existing parent. It is joined by one denylist ([`PROTECTED_DIRS`]) for the
//! two directories that live INSIDE the root but are not the operator's working
//! tree — `.git` and `.ralphy`. The denylist is enforced twice: on the spelling
//! the client sent ([`refuse_protected`]) and on the path the filesystem
//! RESOLVES it to ([`confine_outside_protected`]) — Windows answers `.git.`,
//! `.git ` and the 8.3 short name `GIT~1` with the real `.git`, and an in-root
//! symlink can point at it, so a lexical compare alone is not the boundary.
//! The denylist has exactly one carve-out, [`is_note_in_notes_dir`]: the notes
//! landing directory (ADR-0064 §5), which is the operator's documents rather
//! than daemon state.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use encoding_rs::Encoding;

use crate::confine::{self, ConfineError};
use crate::textcodec;

/// A Write byte-op failure. `Confined` is a refused escape (traversal/symlink),
/// surfaced verbatim (not masked to a miss like reads — a write-escape refusal
/// confirms nothing); `Conflict` is create/rename onto an existing path;
/// `NotFound` is a rename/delete of an absent source; `Unencodable` is a save
/// whose text the named encoding cannot represent; `Io` is any other failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// The target escapes the repo root (traversal or symlink) — refused.
    Confined,
    /// The target already exists (create) or the destination exists (rename).
    Conflict,
    /// The source path does not exist (rename/delete).
    NotFound,
    /// A char at `char_index` (zero-based) has no representation in the
    /// encoding the write named; nothing was written (ADR-0036 amendment
    /// 2026-09-22: a round-trip or a refusal, never a substitution).
    Unencodable { char_index: usize },
    /// An underlying filesystem error.
    Io,
}

impl WriteError {
    /// The refusal as the WIRE spells it — the one table the Write verbs and
    /// the clipboard drop (ADR-0055) both answer with.
    pub fn reason(self) -> &'static str {
        match self {
            WriteError::Confined => "refused",
            WriteError::Conflict => "exists",
            WriteError::NotFound => "not found",
            WriteError::Unencodable { .. } => "unencodable",
            WriteError::Io => "io error",
        }
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Confined => write!(f, "path escapes the repo root"),
            WriteError::Conflict => write!(f, "path already exists"),
            WriteError::NotFound => write!(f, "path not found"),
            WriteError::Unencodable { char_index } => {
                write!(
                    f,
                    "character {char_index} is not representable in the target encoding"
                )
            }
            WriteError::Io => write!(f, "io error"),
        }
    }
}

impl std::error::Error for WriteError {}

/// Map a confinement failure to a Write failure: an escape surfaces verbatim as
/// `Confined`, a missing parent as `NotFound`. Crate-visible so the clipboard
/// drop writer (ADR-0055) refuses in the same vocabulary.
pub(crate) fn map_confine(e: ConfineError) -> WriteError {
    match e {
        ConfineError::Escape => WriteError::Confined,
        ConfineError::NotFound => WriteError::NotFound,
    }
}

/// Directories the Write path never has a legitimate reason to touch, refused
/// wherever they appear in the target. Confinement bounds writes to the repo
/// ROOT, and both of these live inside it: `.git` holds the history a workbench
/// edit must never rewrite (a recursive `delete` of it is unrecoverable), and
/// `.ralphy` is daemon-and-run state the daemon itself reads back as trusted
/// config. Git operations go through the git verbs, not through byte-ops.
const PROTECTED_DIRS: [&str; 2] = [".git", ".ralphy"];

/// Whether one path component names a protected directory. Case-insensitive
/// (NTFS treats `.GIT` as the same directory) and blind to trailing dots and
/// spaces (the Win32 layer strips them, so `.git.` and `.git ` open `.git`). A
/// component that is not UTF-8 is treated as protected: nothing in the
/// workbench produces one, and refusing is the fail-closed answer.
fn is_protected(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return true;
    };
    let name = name.trim_end_matches(['.', ' ']);
    PROTECTED_DIRS.iter().any(|p| name.eq_ignore_ascii_case(p))
}

/// The ONE carve-out in [`PROTECTED_DIRS`] (ADR-0064 §5): a note file sitting
/// directly in the notes landing directory, i.e. exactly the three components
/// `.ralphy` / `notes` / `<name>.note`. It exists because a note is the
/// operator's own document, not daemon state, and the explorer must be able to
/// rename and delete one with the generic byte-ops — so the hole is opened
/// HERE, once, for both gates, rather than by teaching each verb an exception.
///
/// Unlike [`is_protected`] this match is EXACT: no case folding, no trailing
/// dot/space trimming, no subdirectory. A spelling the filesystem would rewrite
/// (`.RALPHY./notes/x.note`) does not open the hole — it falls through to the
/// denylist and is refused. The carve-out is the narrow thing; the denylist is
/// the default.
fn is_note_in_notes_dir(path: &Path) -> bool {
    let parts: Vec<&OsStr> = path.components().map(|c| c.as_os_str()).collect();
    let [dir, sub, name] = parts[..] else {
        return false;
    };
    dir == OsStr::new(".ralphy")
        && sub == OsStr::new("notes")
        && name
            .to_str()
            .is_some_and(|n| n.len() > ".note".len() && n.ends_with(".note"))
}

/// Refuse a target that traverses or names a protected directory, as SPELLED by
/// the client. The resolved-path check in [`confine_outside_protected`] is the
/// one that holds against spellings the filesystem rewrites.
fn refuse_protected(rel: &str) -> Result<(), WriteError> {
    let path = Path::new(rel);
    if is_note_in_notes_dir(path) {
        return Ok(());
    }
    if path.components().any(|c| is_protected(c.as_os_str())) {
        return Err(WriteError::Confined);
    }
    Ok(())
}

/// Confine `rel` for a write AND refuse a target that RESOLVES into a protected
/// directory, whatever it was spelled as — a Windows 8.3 short name (`GIT~1`), a
/// trailing dot (`.git.`), an in-root symlink aimed at `.git`. The lexical
/// denylist judges the request; this judges the filesystem's answer.
///
/// `confine_write` canonicalizes the parent but leaves the final component raw
/// (it may not exist yet), so an existing target is canonicalized here too: a
/// delete or overwrite of `GIT~1` IS the real `.git`. A target that does not
/// exist cannot be an alias of anything — its parent chain is already canonical
/// and its name passed the lexical gate. Only the components BELOW the canonical
/// root are inspected: the repo itself may live under a `.ralphy/worktrees/…`
/// path, and that is not the operator writing into `.ralphy`.
fn confine_outside_protected(root: &Path, rel: &str) -> Result<PathBuf, WriteError> {
    // A `:` inside a name is an NTFS alternate data stream (`a.txt:hidden` writes
    // a stream on `a.txt`, and `.git:x` a stream on the directory) — a second
    // name-rewriting class the lexical compare cannot see. The read side
    // (`dispatch::validated_path`) already refuses `:` everywhere; the write side
    // is at least as strict.
    if rel.contains(':') {
        return Err(WriteError::Confined);
    }
    refuse_protected(rel)?;
    let target = confine::confine_write(root, rel).map_err(map_confine)?;
    let canon_root = root.canonicalize().map_err(|_| WriteError::NotFound)?;
    let resolved = target.canonicalize().unwrap_or_else(|_| target.clone());
    let inside = resolved
        .strip_prefix(&canon_root)
        .map_err(|_| WriteError::Confined)?;
    if !is_note_in_notes_dir(inside) && inside.components().any(|c| is_protected(c.as_os_str())) {
        return Err(WriteError::Confined);
    }
    Ok(target)
}

/// Write `content` to the confined `rel` file under `root` as UTF-8 without a
/// BOM, creating or overwriting it. The parent dir must exist (confinement
/// confines it). The verb uses [`write_encoded`]; this is the shape every
/// caller that writes the daemon's own UTF-8 keeps.
pub fn write(root: &Path, rel: &str, content: &str) -> Result<(), WriteError> {
    write_encoded(root, rel, content, encoding_rs::UTF_8, false)
}

/// [`write`] encoding `content` with `encoding` (a BOM prepended when `bom`,
/// for the encodings that have one) — the bytes a [`crate::tree::read_with`]
/// took come back the same. A char the encoding cannot represent refuses the
/// whole write with [`WriteError::Unencodable`] and touches nothing. This
/// changes WHICH bytes are written, never where: confinement and the denylist
/// gate the path exactly as for [`write`].
pub fn write_encoded(
    root: &Path,
    rel: &str,
    content: &str,
    encoding: &'static Encoding,
    bom: bool,
) -> Result<(), WriteError> {
    // Confined FIRST, encoded second: the module doc's claim is that a denied
    // path is answered by the denylist, and encoding first would answer a
    // refused `.git/…` write with `unencodable` whenever the text also
    // happened to carry a char the named encoding cannot represent.
    let path = confine_outside_protected(root, rel)?;
    let bytes = textcodec::encode(content, encoding, bom)
        .map_err(|char_index| WriteError::Unencodable { char_index })?;
    std::fs::write(&path, bytes).map_err(|_| WriteError::Io)
}

/// [`write`]'s byte direction: `bytes` REPLACE the confined `rel`, through the
/// same confinement and denylist every other op goes through. Crate-visible
/// because the one caller that writes bytes it built itself is the note
/// container ([`crate::note`], ADR-0064) — the Write VERBS all carry text, and
/// nothing on the wire may name a path here without passing
/// [`confine_outside_protected`].
///
/// Written to a sibling and renamed over the target, the shape
/// [`crate::desk::save_to`] already uses. This is not tidiness: a `.note` is
/// all-or-nothing (its container inflates as one stream), so a process killed
/// mid-write would leave not a truncated tail but an unreadable file — the
/// whole note, for a save that happens every 800 ms while a card is open. The
/// temp name is DERIVED from the resolved target, never client-named, so it
/// needs no second confinement; it lands beside the target, inside the same
/// already-confined directory.
pub(crate) fn write_bytes(root: &Path, rel: &str, bytes: &[u8]) -> Result<(), WriteError> {
    let path = confine_outside_protected(root, rel)?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".writing");
    let tmp = path.with_file_name(name);
    std::fs::write(&tmp, bytes).map_err(|_| WriteError::Io)?;
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // The rename is what makes this atomic; a failure leaves the old
            // file intact, and the half-written sibling must not be left on
            // the operator's tree.
            let _ = std::fs::remove_file(&tmp);
            Err(WriteError::Io)
        }
    }
}

/// Create the confined directory `rel` under `root` if it is missing. A file or
/// a symlink squatting on the name is refused, never replaced.
///
/// This deliberately does NOT consult [`PROTECTED_DIRS`]: its two callers — the
/// clipboard drop (ADR-0055) and the note writer (ADR-0064) — create a landing
/// directory the VERB fixes, contributing no client path at all, which is the
/// same argument that lets `plan.discard` name a file inside `.ralphy`. Never
/// `create_dir_all`, which would walk through a symlink it did not check.
pub(crate) fn ensure_dir(root: &Path, rel: &str) -> Result<(), WriteError> {
    let dir = confine::confine_write(root, rel).map_err(map_confine)?;
    match std::fs::create_dir(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if dir.is_dir() {
                Ok(())
            } else {
                Err(WriteError::Conflict)
            }
        }
        Err(_) => Err(WriteError::Io),
    }
}

/// Create the confined `rel` as a directory (`dir`) or a new empty file, refusing
/// with `Conflict` if the path already exists.
pub fn create(root: &Path, rel: &str, dir: bool) -> Result<(), WriteError> {
    let path = confine_outside_protected(root, rel)?;
    if path.exists() {
        return Err(WriteError::Conflict);
    }
    if dir {
        std::fs::create_dir(&path).map_err(|_| WriteError::Io)
    } else {
        // `create_new` refuses an existing file atomically (defence in depth over
        // the `exists()` pre-check, which races).
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map(|_| ())
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AlreadyExists => WriteError::Conflict,
                _ => WriteError::Io,
            })
    }
}

/// Rename the confined `from_rel` to the confined `to_rel` (both under `root`),
/// refusing with `NotFound` if the source is absent and `Conflict` if the
/// destination already exists.
pub fn rename(root: &Path, from_rel: &str, to_rel: &str) -> Result<(), WriteError> {
    let from = confine_outside_protected(root, from_rel)?;
    let to = confine_outside_protected(root, to_rel)?;
    if !from.exists() {
        return Err(WriteError::NotFound);
    }
    if to.exists() {
        return Err(WriteError::Conflict);
    }
    std::fs::rename(&from, &to).map_err(|_| WriteError::Io)
}

/// Copy the confined `from_rel` to the confined `to_rel` (both under `root`),
/// leaving the source in place — the one difference from [`rename`], and what
/// makes it the byte-op behind the explorer's Duplicate.
///
/// A FILE only: anything else (a directory, a symlink) is `Confined`, because a
/// recursive copy is a blast radius no verb here asks for and a symlink source
/// would harvest bytes from outside the root. `symlink_metadata` is what refuses
/// the link rather than following it.
pub fn copy(root: &Path, from_rel: &str, to_rel: &str) -> Result<(), WriteError> {
    let from = confine_outside_protected(root, from_rel)?;
    let to = confine_outside_protected(root, to_rel)?;
    let meta = std::fs::symlink_metadata(&from).map_err(|_| WriteError::NotFound)?;
    if !meta.is_file() {
        return Err(WriteError::Confined);
    }
    // `create_new`, not `fs::copy` after an `exists()` pre-check: `fs::copy`
    // TRUNCATES an existing destination, so the pre-check alone is a check-then-act
    // race whose loss is the operator's file — and `duplicate` picks its name by
    // listing a directory a watcher is concurrently changing, so the window is
    // real. Same defence `create` already takes.
    let mut src = std::fs::File::open(&from).map_err(|_| WriteError::Io)?;
    let mut dst = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&to)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => WriteError::Conflict,
            _ => WriteError::Io,
        })?;
    std::io::copy(&mut src, &mut dst)
        .map(|_| ())
        .map_err(|_| WriteError::Io)
}

/// Delete the confined `rel` under `root`: a directory recursively
/// (`remove_dir_all`), a file with `remove_file`. Confinement already bounds the
/// blast radius to the repo root; a missing target is `NotFound`.
pub fn delete(root: &Path, rel: &str) -> Result<(), WriteError> {
    let path = confine_outside_protected(root, rel)?;
    let meta = std::fs::symlink_metadata(&path).map_err(|_| WriteError::NotFound)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(&path).map_err(|_| WriteError::Io)
    } else {
        std::fs::remove_file(&path).map_err(|_| WriteError::Io)
    }
}

/// The run artifact the operator is allowed to throw away: `.ralphy/plan.md`.
///
/// Why this is a function and not a `delete` call: [`PROTECTED_DIRS`] refuses
/// `.ralphy` on every generic byte-op, and that refusal stays — the client
/// contributes NO path here, so the denylist is not weakened by a hole but
/// bypassed by a target the verb itself fixes (the same shape as `runs.list`,
/// whose ADR-0036 §1 argument is "the verb alone fixes what is read").
///
/// It exists because a finalized plan is picked up by the next run (the
/// `<!-- ralphy-plan: issue=N -->` trailer is the resume signal — see
/// `ralphy_adapter_support::resume`), so changing one's mind about a planned issue
/// meant deleting the file by hand. Absent is `NotFound`, never a silent success:
/// "there was no plan to discard" is a different answer from "the plan is gone",
/// and the panel says which.
///
/// Only ever a regular file. `symlink_metadata`, so a symlink at `plan.md` is
/// refused rather than followed out of the repo, and a directory of that name is
/// refused rather than recursively removed — this path takes no client input, so
/// the one thing it must never grow is a recursive delete.
pub fn discard_plan(root: &Path) -> Result<(), WriteError> {
    let path = root.join(".ralphy").join("plan.md");
    let meta = std::fs::symlink_metadata(&path).map_err(|_| WriteError::NotFound)?;
    if !meta.is_file() {
        return Err(WriteError::Confined);
    }
    std::fs::remove_file(&path).map_err(|_| WriteError::Io)
}

#[cfg(test)]
mod tests {
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
}
