//! The Write byte-op path (ADR-0036 §2, Write effect class): four pure functions
//! over a confined target — [`write`], [`create`], [`rename`], [`delete`]. Like
//! [`crate::tree`] on the read side, this module carries NO repo semantics and
//! does NOT consult the run lock (ADR-0036 amendment: "Write does not consult the
//! run lock" — operator-owns-the-tree). Confinement ([`crate::confine`]) is the
//! security boundary in SPACE: every op resolves its target through
//! [`confine::confine_write`], which confines a maybe-missing target by confining
//! its existing parent. It is joined by one denylist ([`PROTECTED_DIRS`]) for the
//! two directories that live INSIDE the root but are not the operator's working
//! tree — `.git` and `.ralphy`.

use std::path::Path;

use crate::confine::{self, ConfineError};

/// A Write byte-op failure. `Confined` is a refused escape (traversal/symlink),
/// surfaced verbatim (not masked to a miss like reads — a write-escape refusal
/// confirms nothing); `Conflict` is create/rename onto an existing path;
/// `NotFound` is a rename/delete of an absent source; `Io` is any other failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// The target escapes the repo root (traversal or symlink) — refused.
    Confined,
    /// The target already exists (create) or the destination exists (rename).
    Conflict,
    /// The source path does not exist (rename/delete).
    NotFound,
    /// An underlying filesystem error.
    Io,
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Confined => write!(f, "path escapes the repo root"),
            WriteError::Conflict => write!(f, "path already exists"),
            WriteError::NotFound => write!(f, "path not found"),
            WriteError::Io => write!(f, "io error"),
        }
    }
}

impl std::error::Error for WriteError {}

/// Map a confinement failure to a Write failure: an escape surfaces verbatim as
/// `Confined`, a missing parent as `NotFound`.
fn map_confine(e: ConfineError) -> WriteError {
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

/// Refuse a target that traverses or names a protected directory. Compared
/// case-insensitively: NTFS would treat `.GIT` as the same directory.
fn refuse_protected(rel: &str) -> Result<(), WriteError> {
    let names_protected = Path::new(rel).components().any(|c| {
        PROTECTED_DIRS
            .iter()
            .any(|p| c.as_os_str().eq_ignore_ascii_case(p))
    });
    if names_protected {
        return Err(WriteError::Confined);
    }
    Ok(())
}

/// Write `content` to the confined `rel` file under `root`, creating or
/// overwriting it. The parent dir must exist (confinement confines it).
pub fn write(root: &Path, rel: &str, content: &str) -> Result<(), WriteError> {
    refuse_protected(rel)?;
    let path = confine::confine_write(root, rel).map_err(map_confine)?;
    std::fs::write(&path, content).map_err(|_| WriteError::Io)
}

/// Create the confined `rel` as a directory (`dir`) or a new empty file, refusing
/// with `Conflict` if the path already exists.
pub fn create(root: &Path, rel: &str, dir: bool) -> Result<(), WriteError> {
    refuse_protected(rel)?;
    let path = confine::confine_write(root, rel).map_err(map_confine)?;
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
    refuse_protected(from_rel)?;
    refuse_protected(to_rel)?;
    let from = confine::confine_write(root, from_rel).map_err(map_confine)?;
    let to = confine::confine_write(root, to_rel).map_err(map_confine)?;
    if !from.exists() {
        return Err(WriteError::NotFound);
    }
    if to.exists() {
        return Err(WriteError::Conflict);
    }
    std::fs::rename(&from, &to).map_err(|_| WriteError::Io)
}

/// Delete the confined `rel` under `root`: a directory recursively
/// (`remove_dir_all`), a file with `remove_file`. Confinement already bounds the
/// blast radius to the repo root; a missing target is `NotFound`.
pub fn delete(root: &Path, rel: &str) -> Result<(), WriteError> {
    refuse_protected(rel)?;
    let path = confine::confine_write(root, rel).map_err(map_confine)?;
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
        // A name that merely CONTAINS a protected name stays writable.
        write(root.path(), ".gitignore", "target/").unwrap();
        write(root.path(), "gitlab.yml", "x").unwrap();
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
