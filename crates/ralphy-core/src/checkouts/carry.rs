//! Carry-over: what a fresh worktree gets from the primary tree that git does
//! not give it — the gitignored files it needs to be usable (ADR-0063 §6 as
//! amended; ADR-0058 §4's design). Two lists in `.ralphy/settings.json`,
//! both **warn-only**: an entry that is missing, not gitignored, of the wrong
//! kind or that fails to land is reported and skipped, and the add never
//! fails because of it.
//!
//! - `worktree.copy`: literal relative paths that exist in the primary and are
//!   gitignored; a file is copied, a directory copied recursively. Copied,
//!   never linked, so the agent's edits cannot leak back into the primary.
//! - `worktree.share`: directories only, existing and gitignored; linked — a
//!   junction on Windows (no privilege needed), a symlink elsewhere. A share
//!   **never falls back to a copy**: `node_modules` copied is slow and
//!   duplicates disk, which is the thing sharing is for. One that cannot be
//!   linked is skipped with a warning.
//!
//! No setup script and no dotfile: settings are the one configuration
//! surface (rejected alternatives in ADR-0058/0063).

use std::path::{Component, Path, PathBuf};

use crate::git::raw;
use crate::settings::WorktreeSettings;

/// Carry the configured gitignored paths from `primary` into the worktree at
/// `dest`. Returns one warning line per skipped entry — the caller prints
/// them, the add is already done.
pub fn carry_over(primary: &Path, dest: &Path, settings: &WorktreeSettings) -> Vec<String> {
    let mut warnings = Vec::new();
    for entry in &settings.copy {
        if let Err(why) = copy_one(primary, dest, entry) {
            warnings.push(format!("worktree.copy: {entry}: {why}"));
        }
    }
    for entry in &settings.share {
        if let Err(why) = share_one(primary, dest, entry) {
            warnings.push(format!("worktree.share: {entry}: {why}"));
        }
    }
    warnings
}

/// The relative path an entry names, or why it cannot be one: a bare
/// relative path with no `..`, no root and no drive — the lists are literal
/// paths under the primary tree and nothing else.
fn relative(entry: &str) -> Result<PathBuf, String> {
    let trimmed = entry.trim().trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return Err("empty entry".into());
    }
    let path = Path::new(trimmed);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("must be a relative path under the repository".into());
    }
    Ok(path.to_path_buf())
}

/// Whether the primary tree gitignores `rel` — git's own answer, so the rule
/// that ignores it (nested `.gitignore`, `info/exclude`, a global excludes
/// file) does not matter. Exit 0 is ignored, 1 is not, anything else is git
/// refusing to say.
fn ignored(primary: &Path, rel: &Path) -> Result<bool, String> {
    let arg = rel.to_string_lossy();
    let out = raw(primary, &["check-ignore", "-q", "--", &arg]).map_err(|e| format!("{e:#}"))?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "`git check-ignore` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

/// The source in the primary and the target in the worktree for one entry,
/// after the gates every entry shares: a well-formed relative path, present
/// in the primary, gitignored there, and not already present in the worktree
/// (git may have put something there — a tracked path is never overwritten).
fn endpoints(
    primary: &Path,
    dest: &Path,
    entry: &str,
    dir_only: bool,
) -> Result<(PathBuf, PathBuf), String> {
    let rel = relative(entry)?;
    let src = primary.join(&rel);
    if !src.exists() {
        return Err("does not exist in the primary tree, skipped".into());
    }
    if dir_only && !src.is_dir() {
        return Err("is not a directory (share links directories only), skipped".into());
    }
    if !ignored(primary, &rel)? {
        return Err("is not gitignored, skipped".into());
    }
    let target = dest.join(&rel);
    if target.exists() || target.symlink_metadata().is_ok() {
        return Err("already exists in the worktree, skipped".into());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    Ok((src, target))
}

fn copy_one(primary: &Path, dest: &Path, entry: &str) -> Result<(), String> {
    let (src, target) = endpoints(primary, dest, entry, false)?;
    if src.is_dir() {
        copy_dir_all(&src, &target).map_err(|e| format!("copying: {e}"))
    } else {
        std::fs::copy(&src, &target)
            .map(|_| ())
            .map_err(|e| format!("copying: {e}"))
    }
}

fn share_one(primary: &Path, dest: &Path, entry: &str) -> Result<(), String> {
    let (src, target) = endpoints(primary, dest, entry, true)?;
    link_dir(&src, &target).map_err(|e| format!("linking: {e}"))
}

/// A recursive copy that follows nothing: a symlink inside a copied tree is
/// copied as the file it points at (`fs::copy` semantics), which is the
/// conservative reading for a `.vscode/`-sized directory.
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

/// A directory link `target -> src`. Windows: a junction, which any user may
/// create (a symlink needs Developer Mode or a privilege), and which git
/// treats like a directory. Elsewhere: a symlink.
#[cfg(windows)]
fn link_dir(src: &Path, target: &Path) -> std::io::Result<()> {
    junction::create(src, target)
}

#[cfg(not(windows))]
fn link_dir(src: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_accepts_plain_paths_and_refuses_escapes() {
        assert_eq!(relative(".env").unwrap(), PathBuf::from(".env"));
        assert_eq!(relative(".vscode/").unwrap(), PathBuf::from(".vscode"));
        assert_eq!(
            relative("a/b/c").unwrap(),
            PathBuf::from("a").join("b").join("c")
        );
        for bad in ["", "  ", "../x", "a/../b", "/abs", "./x"] {
            assert!(relative(bad).is_err(), "{bad:?} must be refused");
        }
        #[cfg(windows)]
        {
            assert!(relative("C:\\abs").is_err());
            assert!(relative("\\\\server\\share").is_err());
        }
    }

    /// Pure-fs leg: no git, so an unrelated `primary` that is not a repository
    /// answers "not a repository" from `check-ignore` — and the entry is a
    /// WARNING, never a failure.
    #[test]
    fn a_missing_entry_and_a_bad_entry_are_warnings_not_errors() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("p");
        let dest = dir.path().join("w");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        let settings = WorktreeSettings {
            copy: vec!["nope".into(), "../escape".into()],
            share: vec!["".into()],
        };
        let warnings = carry_over(&primary, &dest, &settings);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings[0].contains("does not exist"), "{warnings:?}");
        assert!(warnings[1].contains("relative path"), "{warnings:?}");
        assert!(warnings[2].contains("empty"), "{warnings:?}");
        assert!(
            std::fs::read_dir(&dest).unwrap().next().is_none(),
            "nothing landed"
        );
    }
}
