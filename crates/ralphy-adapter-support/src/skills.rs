//! The `.agents/skills` exposure dance: linking a ralphy-owned skill tree into a
//! shared, operator-owned discovery directory without clobbering what the operator
//! keeps there.
//!
//! Vendor-neutral by construction — every vendor that discovers skills through the
//! conventional `.agents/skills` hierarchy needs the same three primitives, and
//! only the per-skill loop around them differs.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// Link `src` into `dest` as a directory symlink, falling back to a recursive copy
/// when the symlink is rejected on Windows (no Developer Mode / not elevated).
pub fn link_or_copy_dir(src: &Path, dest: &Path) -> Result<()> {
    match symlink_dir(src, dest) {
        Ok(()) => Ok(()),
        Err(_) if cfg!(windows) => copy_dir_all(src, dest)
            .with_context(|| format!("copying {} -> {}", src.display(), dest.display())),
        Err(e) => {
            Err(e).with_context(|| format!("symlinking {} -> {}", src.display(), dest.display()))
        }
    }
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dest: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dest).map_err(Into::into)
}

#[cfg(windows)]
fn symlink_dir(src: &Path, dest: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(src, dest).map_err(Into::into)
}

/// Remove a path that may be a symlink, a real directory, or a file — without
/// following a symlink into its target. On Windows a directory symlink must be
/// removed via `remove_dir`, a file symlink via `remove_file`, so both are tried.
pub fn remove_path(p: &Path) -> Result<()> {
    let ft = fs::symlink_metadata(p)?.file_type();
    if ft.is_symlink() {
        #[cfg(windows)]
        {
            fs::remove_file(p).or_else(|_| fs::remove_dir(p))?;
        }
        #[cfg(unix)]
        {
            fs::remove_file(p)?;
        }
    } else if ft.is_dir() {
        fs::remove_dir_all(p)?;
    } else {
        fs::remove_file(p)?;
    }
    Ok(())
}

/// Recursively copy `src` into `dest` (the Windows fallback when symlinks are
/// unavailable). Creates `dest` and every intermediate directory.
fn copy_dir_all(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Ensure a `/<name>` ignore line exists for each ralphy skill — plus a
/// `/.gitignore` self-ignore — at `path`, appending only what's missing.
///
/// Two invariants this must keep, because the directory is shared with the
/// operator: the `/.gitignore` self-entry is ALWAYS emitted (without it the file
/// is the lone unignored thing left in the directory, so its parent shows as
/// untracked and dirties the working tree, aborting the next run's clean-tree
/// check), and existing lines are NEVER removed or reordered. Idempotent: a no-op
/// once the lines exist.
pub fn ensure_gitignore_entries(path: &Path, names: &[std::ffi::OsString]) -> Result<()> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let mut changed = false;
    let entries = std::iter::once("/.gitignore".to_string())
        .chain(names.iter().map(|n| format!("/{}", n.to_string_lossy())));
    for entry in entries {
        if !lines.iter().any(|l| l.trim() == entry) {
            lines.push(entry);
            changed = true;
        }
    }
    if changed {
        let mut out = lines.join("\n");
        out.push('\n');
        fs::write(path, out).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh temp dir holding `src/nested/deep.txt` = `deep`, plus the `dest`
    /// path (not created) the dance targets.
    fn seeded_tree(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "ralphy-support-skills-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let src = base.join("src");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::write(src.join("nested/deep.txt"), b"deep").unwrap();
        let dest = base.join("dest");
        (base, src, dest)
    }

    #[test]
    fn copy_fallback_reproduces_the_tree() {
        let (base, src, dest) = seeded_tree("copy");
        copy_dir_all(&src, &dest).expect("copy_dir_all");
        assert_eq!(
            fs::read_to_string(dest.join("nested/deep.txt")).unwrap(),
            "deep"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn link_or_copy_dir_resolves_the_same_content() {
        // Drives the real path: the symlink branch on Linux, whichever branch
        // Windows takes. Either way `dest` must resolve to the same content.
        let (base, src, dest) = seeded_tree("link");
        link_or_copy_dir(&src, &dest).expect("link_or_copy_dir");
        assert_eq!(
            fs::read_to_string(dest.join("nested/deep.txt")).unwrap(),
            "deep"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn remove_path_clears_a_link_without_touching_the_target() {
        let (base, src, dest) = seeded_tree("remove");
        link_or_copy_dir(&src, &dest).expect("link_or_copy_dir");
        remove_path(&dest).expect("remove_path");
        assert!(!dest.exists(), "dest must be gone");
        assert!(
            src.join("nested/deep.txt").is_file(),
            "removing the link must not reach the target"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn ensure_gitignore_entries_merges_without_clobbering() {
        let base = std::env::temp_dir().join(format!("ralphy-support-gi-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let path = base.join(".gitignore");
        fs::write(&path, b"my-secret\n").unwrap();

        let names = vec![std::ffi::OsString::from("reviewer")];
        ensure_gitignore_entries(&path, &names).expect("first call");
        ensure_gitignore_entries(&path, &names).expect("second call");

        let gi = fs::read_to_string(&path).unwrap();
        assert!(
            gi.lines().any(|l| l.trim() == "my-secret"),
            "the operator's own line must survive: {gi:?}"
        );
        assert!(
            gi.lines().any(|l| l.trim() == "/.gitignore"),
            "the self-entry must be emitted: {gi:?}"
        );
        assert_eq!(
            gi.lines().filter(|l| l.trim() == "/reviewer").count(),
            1,
            "a second call must not duplicate: {gi:?}"
        );

        let _ = fs::remove_dir_all(&base);
    }
}
