//! Checkouts: the git worktrees Ralphy created under `.ralphy/worktrees/<name>`
//! (ADR-0063 §1). This module is the READ path — [`list`] reports them with
//! their branch, recorded base and dirty flag, and normalises any starting
//! directory (the primary tree or one of the worktrees) to the primary through
//! the git common dir.
//!
//! A worktree the operator made by hand somewhere else is not the workbench's
//! and is not listed; neither is a nested path under the fixed location. The
//! name of a checkout is its directory name and its branch name at once.
//!
//! Not to be confused with [`crate::worktree`], which is the *working-tree
//! operations* (stage/unstage/commit/discard) of one tree.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::git::{git, raw};

/// Where the workbench keeps its worktrees, relative to the primary tree.
pub const WORKTREES_DIR: &str = ".ralphy/worktrees";

/// One workbench worktree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Checkout {
    pub name: String,
    pub path: String,
    /// The checked-out branch; empty when the worktree is detached.
    pub branch: String,
    /// `branch.<name>.base` from the primary tree's config; empty when unset.
    pub base: String,
    pub dirty: bool,
}

/// The primary tree and the workbench worktrees registered on it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Listing {
    pub primary: String,
    pub worktrees: Vec<Checkout>,
}

/// One record of `git worktree list --porcelain -z`, before the filter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Entry {
    pub(crate) path: String,
    pub(crate) branch: String,
    pub(crate) detached: bool,
    pub(crate) prunable: bool,
}

/// Parse `git worktree list --porcelain -z`: every attribute is NUL-terminated
/// and an empty attribute closes the record. `HEAD`, `bare` and `locked` are
/// read and ignored.
pub(crate) fn parse_porcelain(bytes: &[u8]) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut current: Option<Entry> = None;
    for field in bytes.split(|b| *b == 0) {
        if field.is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            continue;
        }
        let field = String::from_utf8_lossy(field);
        let entry = current.get_or_insert_with(Entry::default);
        if let Some(path) = field.strip_prefix("worktree ") {
            entry.path = path.to_string();
        } else if let Some(branch) = field.strip_prefix("branch ") {
            entry.branch = branch
                .strip_prefix("refs/heads/")
                .unwrap_or(branch)
                .to_string();
        } else if field == "detached" {
            entry.detached = true;
        } else if field == "prunable" || field.starts_with("prunable ") {
            entry.prunable = true;
        }
    }
    if let Some(entry) = current.take() {
        entries.push(entry);
    }
    entries
}

/// Keep the entries that are workbench worktrees of `primary`: a direct child
/// of `<primary>/.ralphy/worktrees/` with a non-empty, separator-free name.
/// The first entry is the main worktree (git lists it first) and is skipped;
/// a prunable entry's directory is gone, so it is dropped rather than reported.
pub(crate) fn select_workbench<'a>(
    entries: &'a [Entry],
    primary: &str,
) -> Vec<(String, &'a Entry)> {
    let prefix = format!("{primary}/{WORKTREES_DIR}/");
    entries
        .iter()
        .skip(1)
        .filter(|entry| !entry.prunable)
        .filter_map(|entry| {
            let name = entry.path.strip_prefix(&prefix)?;
            (!name.is_empty() && !name.contains('/')).then(|| (name.to_string(), entry))
        })
        .collect()
}

/// The primary tree of the repository containing `start` — `start` itself when
/// it is inside the primary, the primary when it is inside a linked worktree.
pub fn primary_root(start: &Path) -> Result<PathBuf> {
    let common = git(
        start,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    Path::new(&common)
        .parent()
        .map(Path::to_path_buf)
        .context("git common dir has no parent")
}

/// List the workbench worktrees of the repository containing `start`.
///
/// `primary` is reported in git's own spelling (forward slashes on Windows),
/// and the filter compares that spelling against git's own records, so no path
/// normalisation happens here. Known limit: a worktree the operator added with
/// a differently-cased or 8.3-short drive path would not match the prefix and
/// is left out; workbench-made worktrees are created from the primary's
/// spelling and match by construction.
pub fn list(start: &Path) -> Result<Listing> {
    let primary = primary_root(start)?;
    let out = raw(&primary, &["worktree", "list", "--porcelain", "-z"])?;
    if !out.status.success() {
        bail!(
            "`git worktree list --porcelain -z` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let entries = parse_porcelain(&out.stdout);
    let Some(main) = entries.first() else {
        bail!("git listed no worktree for {}", primary.display());
    };
    let primary_path = main.path.clone();
    let mut worktrees = Vec::new();
    for (name, entry) in select_workbench(&entries, &primary_path) {
        let base = base_of(&primary, &name)?;
        let dirty = !crate::git::is_clean_ignoring_ralphy(Path::new(&entry.path))?;
        worktrees.push(Checkout {
            name,
            path: entry.path.clone(),
            branch: if entry.detached {
                String::new()
            } else {
                entry.branch.clone()
            },
            base,
            dirty,
        });
    }
    Ok(Listing {
        primary: primary_path,
        worktrees,
    })
}

/// `branch.<name>.base` from the primary's config. `git config --get` exits 1
/// when the key is unset — that is an empty base, not a failure.
fn base_of(primary: &Path, name: &str) -> Result<String> {
    let key = format!("branch.{name}.base");
    let out = raw(primary, &["config", "--get", &key])?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    if out.status.code() == Some(1) {
        return Ok(String::new());
    }
    bail!(
        "`git config --get {key}` failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> Entry {
        Entry {
            path: path.to_string(),
            ..Entry::default()
        }
    }

    #[test]
    fn parse_porcelain_reads_nul_records() {
        let bytes = b"worktree C:/r\0HEAD abc\0branch refs/heads/main\0\0\
worktree C:/r/.ralphy/worktrees/wt-a\0HEAD abc\0branch refs/heads/wt-a\0\0\
worktree C:/r/other\0HEAD abc\0detached\0\0\
worktree C:/r/.ralphy/worktrees/gone\0HEAD abc\0branch refs/heads/gone\0\
prunable gitdir file points to non-existent location\0\0";
        let entries = parse_porcelain(bytes);
        assert_eq!(entries.len(), 4, "four records: {entries:?}");
        assert_eq!(entries[0].path, "C:/r");
        assert_eq!(entries[0].branch, "main");
        assert_eq!(entries[1].branch, "wt-a");
        assert!(entries[2].detached && entries[2].branch.is_empty());
        assert!(!entries[2].prunable);
        assert!(entries[3].prunable);
        assert_eq!(entries[3].branch, "gone");
    }

    #[test]
    fn select_workbench_keeps_only_single_segment_children() {
        let mut gone = entry("C:/r/.ralphy/worktrees/gone");
        gone.prunable = true;
        let entries = vec![
            entry("C:/r"),
            entry("C:/r/.ralphy/worktrees/wt-a"),
            entry("C:/r/.ralphy/worktrees/nested/x"),
            entry("C:/r/other"),
            gone,
            entry("C:/r/.ralphy/worktrees/"),
        ];
        let names: Vec<String> = select_workbench(&entries, "C:/r")
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, vec!["wt-a".to_string()]);
    }

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ralphy-checkouts-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn configure(dir: &Path) {
        git(dir, &["config", "user.email", "t@example.com"]).unwrap();
        git(dir, &["config", "user.name", "Test"]).unwrap();
        git(dir, &["config", "core.autocrlf", "false"]).unwrap();
    }

    fn commit_file(dir: &Path, file: &str, body: &str, msg: &str) {
        std::fs::write(dir.join(file), body).unwrap();
        git(dir, &["add", "."]).unwrap();
        git(dir, &["commit", "-q", "-m", msg]).unwrap();
    }

    /// ONE fixture for the whole git-backed leg (the suite is spawn-bound):
    /// `wt-a` dirty with a recorded base, `wt-b` clean with none, and a
    /// worktree elsewhere that must not be reported.
    #[test]
    fn list_reports_only_the_workbench_worktrees() {
        let root = tmp("list");
        let elsewhere = tmp("list-elsewhere");
        let _ = std::fs::remove_dir_all(&elsewhere);
        git(&root, &["init", "-q", "-b", "main"]).unwrap();
        configure(&root);
        commit_file(&root, "README.md", "hello\n", "init");
        commit_file(&root, ".gitignore", ".ralphy/\n", "ignore the run dir");
        std::fs::create_dir_all(root.join(WORKTREES_DIR)).unwrap();
        let wt_a = format!("{WORKTREES_DIR}/wt-a");
        let wt_b = format!("{WORKTREES_DIR}/wt-b");
        git(
            &root,
            &["worktree", "add", "--no-track", "-b", "wt-a", &wt_a, "main"],
        )
        .unwrap();
        git(&root, &["config", "branch.wt-a.base", "main"]).unwrap();
        git(
            &root,
            &["worktree", "add", "--no-track", "-b", "wt-b", &wt_b, "main"],
        )
        .unwrap();
        let elsewhere_str = elsewhere.to_string_lossy().to_string();
        git(
            &root,
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                "elsewhere",
                &elsewhere_str,
                "main",
            ],
        )
        .unwrap();
        let wt_a_path = root.join(WORKTREES_DIR).join("wt-a");
        std::fs::write(wt_a_path.join("scratch.txt"), "dirty\n").unwrap();

        let listing = list(&root).unwrap();
        let names: Vec<&str> = listing.worktrees.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, vec!["wt-a", "wt-b"], "listing: {listing:?}");
        let a = &listing.worktrees[0];
        assert_eq!(a.branch, "wt-a");
        assert_eq!(a.base, "main");
        assert!(a.dirty, "wt-a holds an untracked scratch.txt");
        assert!(
            a.path.ends_with("/.ralphy/worktrees/wt-a"),
            "path in git's spelling: {}",
            a.path
        );
        let b = &listing.worktrees[1];
        assert_eq!(b.branch, "wt-b");
        assert_eq!(b.base, "", "no branch.wt-b.base was recorded");
        assert!(!b.dirty);
        assert!(!listing.worktrees.iter().any(|w| w.name == "elsewhere"));
        assert_eq!(
            listing.primary,
            git(&root, &["rev-parse", "--show-toplevel"]).unwrap()
        );

        // From inside a worktree the common dir leads back to the primary.
        assert_eq!(list(&wt_a_path).unwrap(), listing);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }
}
