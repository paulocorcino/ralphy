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
mod tests;
