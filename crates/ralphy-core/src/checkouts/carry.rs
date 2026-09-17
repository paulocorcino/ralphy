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
    // A source that CONTAINS the worktree (`.ralphy`, or the primary root
    // spelled as a component) would copy itself into itself until a
    // path-length error; a link would make the worktree hold its own parent.
    if dest.starts_with(&src) {
        return Err("contains the worktree itself, skipped".into());
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
    link_dir(&src, &target).map_err(|e| format!("linking: {e}"))?;
    let rel = relative(entry)?;
    ensure_excluded(dest, &rel)
}

/// Keep a freshly linked share out of the worktree's status. `node_modules/`
/// — the trailing slash is the universal spelling — ignores a DIRECTORY,
/// and on Unix the share is a symlink, which git classes as a file: the
/// link sat in the worktree as an untracked entry, the tree read dirty,
/// `remove` refused it and `git add -A` would have committed the link
/// (measured on the Linux CI runner, 2026-09-17; a junction on Windows is a
/// directory to git and never had the problem). So after linking, ask the
/// WORKTREE whether it ignores the link and, when it does not, add the
/// path to `info/exclude` — the repository-local excludes file git shares
/// across its worktrees and never commits. The primary already ignores the
/// real directory, so the extra line changes nothing there. Idempotent: a
/// line already present is not written twice.
fn ensure_excluded(dest: &Path, rel: &Path) -> Result<(), String> {
    if ignored(dest, rel)? {
        return Ok(());
    }
    let out =
        raw(dest, &["rev-parse", "--git-path", "info/exclude"]).map_err(|e| format!("{e:#}"))?;
    if !out.status.success() {
        return Err(format!(
            "locating info/exclude: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let exclude = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let exclude = dest.join(exclude);
    let pattern = format!(
        "/{}",
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    );
    let current = match std::fs::read_to_string(&exclude) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("reading {}: {e}", exclude.display())),
    };
    if current.lines().any(|line| line.trim() == pattern) {
        return Ok(());
    }
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(&pattern);
    next.push('\n');
    std::fs::write(&exclude, next).map_err(|e| format!("writing {}: {e}", exclude.display()))
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

/// Unlink every directory link inside `dest` before the tree is removed.
///
/// Measured (2026-09-16): `git worktree remove` on Windows FOLLOWS an NTFS
/// junction and deletes the primary's `node_modules` through it — the one
/// outcome `worktree.share` must never have. So the workbench unlinks its
/// links first, itself: the reparse point goes, the target stays. Walks the
/// whole tree (a share may be nested: `packages/a/node_modules`), never
/// descends into a link, and skips the `.git` pointer file. Warn-only like
/// the rest of carry-over: a link that cannot be removed is reported, and
/// the caller decides whether to go on.
pub fn unlink_shares(dest: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    unlink_in(dest, &mut warnings);
    warnings
}

fn unlink_in(dir: &Path, warnings: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warnings.push(format!("could not read {}: {e}", dir.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if is_dir_link(&path, &meta) {
            if let Err(e) = remove_dir_link(&path) {
                warnings.push(format!("could not unlink {}: {e}", path.display()));
            }
        } else if meta.is_dir() {
            unlink_in(&path, warnings);
        }
    }
}

/// Whether `path` is a directory link — a symlink to a directory, or on
/// Windows a junction (which `std` reports as a symlink too, but the crate
/// that made it is the one to ask).
fn is_dir_link(path: &Path, meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    is_junction(path)
}

#[cfg(windows)]
fn is_junction(path: &Path) -> bool {
    junction::exists(path).unwrap_or(false)
}

#[cfg(not(windows))]
fn is_junction(_path: &Path) -> bool {
    false
}

/// Remove the link, never what it points at.
#[cfg(windows)]
fn remove_dir_link(path: &Path) -> std::io::Result<()> {
    // A junction or a directory symlink is a directory entry with a reparse
    // point: `remove_dir` deletes the entry and leaves the target alone.
    std::fs::remove_dir(path)
}

#[cfg(not(windows))]
fn remove_dir_link(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
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

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git (CI and the build machine have git)");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The Unix-symlink gap, forced on every platform: a share whose name
    /// the worktree does NOT ignore is written to the repository's
    /// `info/exclude` (the common one, shared by every worktree), the
    /// worktree ignores it afterwards, and a second call writes nothing.
    #[test]
    fn ensure_excluded_writes_the_shared_info_exclude_once() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("p");
        std::fs::create_dir_all(&primary).unwrap();
        git(&primary, &["init", "-q", "-b", "main"]);
        git(&primary, &["config", "user.email", "t@example.com"]);
        git(&primary, &["config", "user.name", "Test"]);
        git(&primary, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let wt = dir.path().join("wt");
        git(
            &primary,
            &["worktree", "add", "-q", &wt.to_string_lossy(), "-b", "wt"],
        );
        let rel = Path::new("node_modules");
        assert!(!ignored(&wt, rel).unwrap(), "nothing ignores it yet");

        ensure_excluded(&wt, rel).unwrap();
        assert!(ignored(&wt, rel).unwrap(), "the worktree ignores it now");
        assert!(
            ignored(&primary, rel).unwrap(),
            "info/exclude is the common one: the primary sees the line too"
        );
        let exclude = primary.join(".git").join("info").join("exclude");
        let text = std::fs::read_to_string(&exclude).unwrap();
        assert_eq!(text.lines().filter(|l| *l == "/node_modules").count(), 1);

        ensure_excluded(&wt, rel).unwrap();
        assert_eq!(
            std::fs::read_to_string(&exclude).unwrap(),
            text,
            "a second call is a no-op"
        );

        // An already-ignored share never touches the file.
        std::fs::write(primary.join(".gitignore"), "dist\n").unwrap();
        git(&primary, &["add", ".gitignore"]);
        git(&primary, &["commit", "-q", "-m", "ignore"]);
        git(&wt, &["merge", "-q", "main"]);
        ensure_excluded(&wt, Path::new("dist")).unwrap();
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), text);
    }

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

    /// A source that contains the destination is refused before any copy —
    /// `.ralphy` holds `.ralphy/worktrees/<wt>`, and copying it would recurse
    /// into itself. Pure-fs: the gate fires before `check-ignore` is asked.
    #[test]
    fn a_source_containing_the_worktree_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("p");
        let dest = primary.join(".ralphy").join("worktrees").join("wt");
        std::fs::create_dir_all(&dest).unwrap();
        let settings = WorktreeSettings {
            copy: vec![".ralphy".into()],
            share: vec![".ralphy/worktrees".into()],
        };
        let warnings = carry_over(&primary, &dest, &settings);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings
                .iter()
                .all(|w| w.contains("contains the worktree itself")),
            "{warnings:?}"
        );
        assert!(!dest.join(".ralphy").exists(), "nothing copied into itself");
    }

    /// `unlink_shares` removes the link and only the link — the target's
    /// files survive — walks into plain directories for nested shares, and
    /// leaves real directories and files alone.
    #[test]
    fn unlink_shares_removes_links_and_keeps_their_targets() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir_all(target.join("pkg")).unwrap();
        std::fs::write(target.join("pkg").join("i.js"), "KEEP").unwrap();
        let wt = dir.path().join("wt");
        std::fs::create_dir_all(wt.join("packages").join("a")).unwrap();
        std::fs::write(wt.join("README.md"), "tracked").unwrap();
        std::fs::write(wt.join(".git"), "gitdir: elsewhere").unwrap();
        link_dir(&target, &wt.join("node_modules")).unwrap();
        link_dir(&target, &wt.join("packages").join("a").join("node_modules")).unwrap();

        let warnings = unlink_shares(&wt);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(
            wt.join("node_modules").symlink_metadata().is_err(),
            "top link gone"
        );
        assert!(
            wt.join("packages")
                .join("a")
                .join("node_modules")
                .symlink_metadata()
                .is_err(),
            "nested link gone"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("pkg").join("i.js")).unwrap(),
            "KEEP"
        );
        assert!(wt.join("README.md").is_file() && wt.join("packages").join("a").is_dir());
        assert!(wt.join(".git").is_file(), "the pointer file is not touched");
    }
}
