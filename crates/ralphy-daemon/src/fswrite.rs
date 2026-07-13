//! The Write byte-op path (ADR-0036 §2, Write effect class): four pure functions
//! over a confined target — [`write`], [`create`], [`rename`], [`delete`]. Like
//! [`crate::tree`] on the read side, this module carries NO repo semantics and
//! does NOT consult the run lock (ADR-0036 amendment: "Write does not consult the
//! run lock" — operator-owns-the-tree). Confinement ([`crate::confine`]) is the
//! ONLY security boundary; every op resolves its target through
//! [`confine::confine_write`], which confines a maybe-missing target by confining
//! its existing parent.

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

/// Write `content` to the confined `rel` file under `root`, creating or
/// overwriting it. The parent dir must exist (confinement confines it).
pub fn write(root: &Path, rel: &str, content: &str) -> Result<(), WriteError> {
    let path = confine::confine_write(root, rel).map_err(map_confine)?;
    std::fs::write(&path, content).map_err(|_| WriteError::Io)
}

/// Create the confined `rel` as a directory (`dir`) or a new empty file, refusing
/// with `Conflict` if the path already exists.
pub fn create(root: &Path, rel: &str, dir: bool) -> Result<(), WriteError> {
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
    let path = confine::confine_write(root, rel).map_err(map_confine)?;
    let meta = std::fs::symlink_metadata(&path).map_err(|_| WriteError::NotFound)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(&path).map_err(|_| WriteError::Io)
    } else {
        std::fs::remove_file(&path).map_err(|_| WriteError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn write_creates_and_overwrites() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "note.txt", "hi").unwrap();
        assert_eq!(fs::read_to_string(root.path().join("note.txt")).unwrap(), "hi");
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
        assert_eq!(create(root.path(), "newdir", true), Err(WriteError::Conflict));
        create(root.path(), "f.txt", false).unwrap();
        assert_eq!(create(root.path(), "f.txt", false), Err(WriteError::Conflict));
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
