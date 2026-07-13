//! The Observe read path's file/tree reader (ADR-0036 §4). Two pure functions
//! over a confined path: [`list`] returns one directory level, gitignore-aware
//! and never descending noise; [`read`] returns a file's text or refuses a
//! binary / oversized file. Confinement ([`crate::confine`]) is the security
//! boundary; the gitignore filter here is UX cleanliness only (ADR-0036 §5) — a
//! gitignored-but-named file is still readable via [`read`], by design.

use std::path::Path;

use crate::confine::{self, ConfineError};

/// Directory-listing hard-exclude: noise dirs never surfaced in the tree. Some
/// are NOT gitignored (`.git`), so `WalkBuilder`'s git filters alone miss them.
const HARD_EXCLUDE: &[&str] = &["node_modules", "target", ".git", ".ralphy"];

/// One tree entry: a child of the listed directory.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
}

/// List the one-level children of the confined `rel` directory under `root`,
/// gitignore-filtered and with [`HARD_EXCLUDE`] noise dirs dropped. Entries are
/// sorted dirs-first, then by name. A confinement failure (escape/missing)
/// propagates as [`ConfineError`].
pub fn list(root: &Path, rel: &str) -> Result<Vec<Entry>, ConfineError> {
    let dir = confine::confine(root, rel)?;

    let mut entries: Vec<Entry> = ignore::WalkBuilder::new(&dir)
        .max_depth(Some(1))
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .hidden(false)
        .filter_entry(|e| {
            e.file_name()
                .to_str()
                .map(|n| !HARD_EXCLUDE.contains(&n))
                .unwrap_or(true)
        })
        .build()
        .filter_map(Result::ok)
        // `max_depth(Some(1))` still yields the root dir itself at depth 0; drop it.
        .filter(|e| e.depth() > 0)
        .map(|e| Entry {
            name: e.file_name().to_string_lossy().into_owned(),
            dir: e.file_type().map(|t| t.is_dir()).unwrap_or(false),
        })
        .collect();

    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

/// A [`read`] failure that is not a plain confinement escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    /// The file contains a NUL byte or invalid UTF-8 in its first window — the
    /// daemon serves text, so a binary file is refused.
    Binary,
    /// The file exceeds [`MAX_READ_BYTES`].
    TooLarge,
    /// The file does not exist or escapes the root (an out-of-root read is
    /// reported as a plain miss, never leaking whether the target exists).
    NotFound,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Binary => write!(f, "binary file"),
            ReadError::TooLarge => write!(f, "file too large"),
            ReadError::NotFound => write!(f, "file not found"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Hard cap on a single [`read`]; the daemon serves bytes, so one read is bounded.
pub const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

/// Window scanned for the text/binary heuristic.
const SNIFF_BYTES: usize = 8 * 1024;

/// Read the confined `rel` file under `root` as text. Refuses a binary file
/// (NUL byte or invalid UTF-8 in the first [`SNIFF_BYTES`]) with
/// [`ReadError::Binary`], a file over [`MAX_READ_BYTES`] with
/// [`ReadError::TooLarge`], and a missing/out-of-root target with
/// [`ReadError::NotFound`] (an escape is masked as a miss, never leaking existence).
pub fn read(root: &Path, rel: &str) -> Result<String, ReadError> {
    let path = confine::confine(root, rel).map_err(|_| ReadError::NotFound)?;
    let meta = std::fs::metadata(&path).map_err(|_| ReadError::NotFound)?;
    if meta.len() > MAX_READ_BYTES {
        return Err(ReadError::TooLarge);
    }
    let bytes = std::fs::read(&path).map_err(|_| ReadError::NotFound)?;
    // NUL in the first window is the cheap binary tell. UTF-8 validity is decided
    // by the WHOLE-file check below, NOT the window: a valid UTF-8 file whose
    // 8 KiB boundary splits a multibyte char would false-positive as binary if
    // the window were UTF-8-checked on its own.
    if bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0) {
        return Err(ReadError::Binary);
    }
    String::from_utf8(bytes).map_err(|_| ReadError::Binary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn list_filters_noise() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("visible.txt"), b"x").unwrap();
        fs::create_dir(root.path().join("node_modules")).unwrap();
        fs::write(root.path().join("node_modules/x"), b"x").unwrap();
        let names: Vec<String> = list(root.path(), "")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(names.contains(&"visible.txt".to_string()));
        assert!(!names.contains(&"node_modules".to_string()));
    }

    #[test]
    fn read_refuses_binary() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("bin.dat"), [0x00, 0x01]).unwrap();
        assert_eq!(read(root.path(), "bin.dat"), Err(ReadError::Binary));
    }

    #[test]
    fn read_refuses_oversized() {
        let root = tempfile::tempdir().unwrap();
        let big = vec![b'a'; (MAX_READ_BYTES + 1) as usize];
        fs::write(root.path().join("big.txt"), &big).unwrap();
        assert_eq!(read(root.path(), "big.txt"), Err(ReadError::TooLarge));
    }

    #[test]
    fn read_returns_text() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("f.txt"), b"hello world").unwrap();
        assert_eq!(read(root.path(), "f.txt"), Ok("hello world".to_string()));
    }

    #[test]
    fn read_accepts_large_utf8_split_at_window_boundary() {
        // A valid UTF-8 file whose byte at SNIFF_BYTES lands mid-multibyte-char
        // must NOT be misread as binary (the window UTF-8 check regression).
        let root = tempfile::tempdir().unwrap();
        // 'é' is 2 bytes; pad so a char straddles the 8 KiB boundary.
        let mut s = "a".repeat(SNIFF_BYTES - 1);
        s.push('é');
        s.push_str(&"b".repeat(100));
        fs::write(root.path().join("big.txt"), s.as_bytes()).unwrap();
        assert_eq!(read(root.path(), "big.txt"), Ok(s));
    }

    #[test]
    fn read_masks_escape_as_not_found() {
        // Security: an out-of-root read must be indistinguishable from a plain
        // miss — `Escape` collapses to `NotFound`, never leaking existence.
        let root = tempfile::tempdir().unwrap();
        assert_eq!(read(root.path(), "../secret"), Err(ReadError::NotFound));
    }
}
