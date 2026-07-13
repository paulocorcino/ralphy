//! Path confinement (ADR-0036 §5): the ONLY security boundary of the Observe
//! read path. A browser names a repo-relative `rel`; [`confine`] resolves it
//! against the repo `root` and returns the resolved path ONLY when it provably
//! stays inside the root — every escape vector (`..` traversal, an absolute
//! `rel` that would replace the base, a symlink whose target is outside) yields
//! [`ConfineError::Escape`]. Gitignore filtering (`crate::tree`) is UX
//! cleanliness, never a security control; this module is.
//!
//! The kernel is reject-then-canonicalize-then-component-prefix-check:
//!
//! 1. Reject any `rel` that `is_absolute()`, is `RootDir`/`Prefix`-anchored, or
//!    carries a `..` component BEFORE joining — `root.join(abs)` silently
//!    DISCARDS the base (`Path::join` semantics), and a `..` whose target does
//!    not exist would fail `canonicalize` (masking a traversal as `NotFound`)
//!    before the range-check runs, so `..` is caught lexically here.
//! 2. `canonicalize` BOTH the root and `root.join(rel)`. Canonicalize resolves
//!    symlinks (so an escaping link's target is what gets range-checked) and
//!    normalizes case / separators / the Windows `\\?\` verbatim prefix
//!    IDENTICALLY on both sides.
//! 3. Assert the resolved path `starts_with` the canonical root COMPONENT-WISE
//!    (iterate [`std::path::Component`]s), never a raw string prefix — a string
//!    prefix would let `/repo-secret` match `/repo`.

use std::path::{Component, Path, PathBuf};

/// A confinement outcome that is NOT a resolved in-root path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfineError {
    /// The requested path resolves outside the repo root (traversal, absolute
    /// `rel`, or a symlink whose target escapes) — refused.
    Escape,
    /// The root or the requested path does not exist. `canonicalize` requires
    /// the path to exist (std semantics), so a missing target lands here.
    NotFound,
}

impl std::fmt::Display for ConfineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfineError::Escape => write!(f, "path escapes the repo root"),
            ConfineError::NotFound => write!(f, "path not found"),
        }
    }
}

impl std::error::Error for ConfineError {}

/// Resolve `rel` against `root`, returning the canonical path ONLY when it stays
/// inside `root`. See the module docs for the kernel; every escape is
/// [`ConfineError::Escape`], a missing root/target is [`ConfineError::NotFound`].
pub fn confine(root: &Path, rel: &str) -> Result<PathBuf, ConfineError> {
    let rel_path = Path::new(rel);
    // Reject an absolute/root/prefix-anchored `rel` (else `root.join(rel)`
    // replaces the base) OR any `..` component up front. `..` must be caught
    // LEXICALLY, not left to the post-canonicalize range-check: a `../secret`
    // whose target does not exist makes `canonicalize` fail with `NotFound`
    // before the range-check runs, masking a traversal as a plain miss.
    if rel_path.is_absolute()
        || rel_path.components().any(|c| {
            matches!(
                c,
                Component::RootDir | Component::Prefix(_) | Component::ParentDir
            )
        })
    {
        return Err(ConfineError::Escape);
    }

    let canon_root = root.canonicalize().map_err(|_| ConfineError::NotFound)?;
    let resolved = root
        .join(rel_path)
        .canonicalize()
        .map_err(|_| ConfineError::NotFound)?;

    // Component-wise prefix: the resolved path must begin with the canonical
    // root's components. `starts_with` on `Path` IS component-wise (not string),
    // and both sides carry the same normalization (case, separators, `\\?\`).
    if resolved.starts_with(&canon_root) {
        Ok(resolved)
    } else {
        Err(ConfineError::Escape)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn refuses_parent_traversal() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(confine(root.path(), "../secret"), Err(ConfineError::Escape));
    }

    #[test]
    fn refuses_absolute_rel() {
        let root = tempfile::tempdir().unwrap();
        let abs = root.path().to_string_lossy().to_string();
        assert_eq!(confine(root.path(), &abs), Err(ConfineError::Escape));
    }

    #[test]
    fn allows_in_root_file() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("sub")).unwrap();
        fs::write(root.path().join("sub/f.txt"), b"hi").unwrap();
        let got = confine(root.path(), "sub/f.txt").expect("in-root file confines");
        assert!(got.ends_with("f.txt"));
        assert!(got.starts_with(root.path().canonicalize().unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_escape() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("target.txt"), b"secret").unwrap();
        // A link inside root pointing at an outside file: canonicalize follows it,
        // so the range-check runs on the outside target and refuses.
        symlink(
            outside.path().join("target.txt"),
            root.path().join("link.txt"),
        )
        .unwrap();
        assert_eq!(confine(root.path(), "link.txt"), Err(ConfineError::Escape));
    }

    #[cfg(windows)]
    #[test]
    fn mixed_case_backslash_resolves_in_root() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("sub")).unwrap();
        fs::write(root.path().join("sub/f.txt"), b"hi").unwrap();
        // Windows canonicalize normalizes case + separators identically on both
        // sides (both carry the `\\?\` verbatim prefix), so a mixed-case,
        // backslash `rel` confines to the same path as the canonical form.
        let canonical = confine(root.path(), "sub/f.txt").expect("canonical form confines");
        let mixed = confine(root.path(), "SUB\\F.TXT").expect("mixed-case form confines");
        assert_eq!(mixed, canonical);
    }
}
