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

/// Confine a MAYBE-MISSING write target. Unlike [`confine`] — which canonicalizes
/// the whole path and so requires it to EXIST — a create/rename-dst target does
/// not yet exist, and full `canonicalize` would fail with `NotFound`, MASKING a
/// traversal as a plain miss (#194's trap). So confine the existing PARENT dir
/// (which [`confine`] canonicalizes, catching parent-chain traversal + symlink
/// escape), append the final component WITHOUT canonicalizing it, and refuse a
/// final component that already exists AS A SYMLINK (`symlink_metadata`, no follow)
/// — writing or deleting THROUGH such a link could redirect outside the root.
pub fn confine_write(root: &Path, rel: &str) -> Result<PathBuf, ConfineError> {
    let rel_path = Path::new(rel);
    // Same lexical rejects as `confine`, plus an empty `rel` (no target to write).
    if rel.is_empty()
        || rel_path.is_absolute()
        || rel_path.components().any(|c| {
            matches!(
                c,
                Component::RootDir | Component::Prefix(_) | Component::ParentDir
            )
        })
    {
        return Err(ConfineError::Escape);
    }

    // Split final component from its parent; a `rel` with no file name (`.`) escapes.
    let name = rel_path.file_name().ok_or(ConfineError::Escape)?;
    let parent = rel_path.parent().unwrap_or_else(|| Path::new(""));
    let parent_str = parent.to_str().ok_or(ConfineError::Escape)?;
    // Confine the parent (must exist); an empty parent resolves to the root itself.
    let confined_parent = confine(root, parent_str)?;
    let target = confined_parent.join(name);

    // The final component is not canonicalized (it may not exist yet), so an
    // existing final SYMLINK could point outside — lstat-reject it here.
    if let Ok(meta) = std::fs::symlink_metadata(&target) {
        if meta.file_type().is_symlink() {
            return Err(ConfineError::Escape);
        }
    }
    Ok(target)
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

    #[test]
    fn confine_write_allows_new_in_root_path() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("sub")).unwrap();
        // `sub/new.txt` does not exist yet, but `sub/` does — it confines.
        let got = confine_write(root.path(), "sub/new.txt").expect("new in-root path confines");
        assert!(got.starts_with(root.path().canonicalize().unwrap()));
        assert!(got.ends_with("new.txt"));
    }

    #[test]
    fn confine_write_refuses_parent_traversal() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            confine_write(root.path(), "../x"),
            Err(ConfineError::Escape)
        );
    }

    #[test]
    fn confine_write_refuses_empty() {
        let root = tempfile::tempdir().unwrap();
        assert!(confine_write(root.path(), "").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn confine_write_refuses_final_symlink() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        // An in-root `link` pointing at an outside dir: writing THROUGH it (as a
        // parent or as the final component) must be refused.
        symlink(outside.path(), root.path().join("link")).unwrap();
        assert_eq!(
            confine_write(root.path(), "link/x"),
            Err(ConfineError::Escape)
        );
        assert_eq!(confine_write(root.path(), "link"), Err(ConfineError::Escape));
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
