//! Checkouts: the git worktrees Ralphy created under `.ralphy/worktrees/<name>`
//! (ADR-0063 §1). This module is the READ path — [`list`] reports them with
//! their branch, recorded base and dirty flag, from any starting directory
//! (the primary tree or one of the worktrees): git lists a repository's
//! worktrees the same from every tree of it, main tree first.
//!
//! A worktree the operator made by hand somewhere else is not the workbench's
//! and is not listed; neither is a nested path under the fixed location. The
//! name of a checkout is its directory name and its branch name at once.
//!
//! Not to be confused with [`crate::worktree`], which is the *working-tree
//! operations* (stage/unstage/commit/discard) of one tree.

use std::path::Path;

use anyhow::{bail, Result};

use crate::git::raw;

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

/// One record of `git worktree list --porcelain`, before the filter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Entry {
    pub(crate) path: String,
    pub(crate) branch: String,
    pub(crate) detached: bool,
    pub(crate) prunable: bool,
}

/// Parse `git worktree list --porcelain`: one attribute per line, a blank line
/// closes the record. The newline form and not `-z`: `-z` needs git 2.36 and
/// the operator's WSL peer ships Ubuntu 22.04's 2.34, while a path with a
/// newline in it is one the workbench never creates. `HEAD`, `bare` and
/// `locked` are read and ignored.
pub(crate) fn parse_porcelain(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut current: Option<Entry> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            continue;
        }
        let entry = current.get_or_insert_with(Entry::default);
        if let Some(path) = line.strip_prefix("worktree ") {
            entry.path = path.to_string();
        } else if let Some(branch) = line.strip_prefix("branch ") {
            entry.branch = branch
                .strip_prefix("refs/heads/")
                .unwrap_or(branch)
                .to_string();
        } else if line == "detached" {
            entry.detached = true;
        } else if line == "prunable" || line.starts_with("prunable ") {
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
/// A prunable entry's directory is gone, so it is dropped rather than reported.
pub(crate) fn select_workbench<'a>(
    entries: &'a [Entry],
    primary: &str,
) -> Vec<(String, &'a Entry)> {
    let prefix = format!("{primary}/{WORKTREES_DIR}/");
    entries
        .iter()
        .filter(|entry| !entry.prunable)
        .filter_map(|entry| {
            let name = entry.path.strip_prefix(&prefix)?;
            (!name.is_empty() && !name.contains('/')).then(|| (name.to_string(), entry))
        })
        .collect()
}

/// List the workbench worktrees of the repository containing `start`.
///
/// `primary` is the first record of git's own listing — the main worktree, in
/// git's own spelling (forward slashes on Windows) — and the filter compares
/// that spelling against the other records, so no path normalisation happens
/// here. Known limit: a worktree the operator added with a differently-cased
/// or 8.3-short drive path would not match the prefix and is left out;
/// workbench-made worktrees are created from the primary's spelling and match
/// by construction. A worktree whose directory is gone but that git still
/// lists (a locked one is never `prunable`) is skipped, not an error.
pub fn list(start: &Path) -> Result<Listing> {
    let out = raw(start, &["worktree", "list", "--porcelain"])?;
    if !out.status.success() {
        bail!(
            "`git worktree list --porcelain` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let entries = parse_porcelain(&String::from_utf8_lossy(&out.stdout));
    let Some(main) = entries.first() else {
        bail!("git listed no worktree for {}", start.display());
    };
    let primary_path = main.path.clone();
    let primary = Path::new(&primary_path);
    let mut worktrees = Vec::new();
    for (name, entry) in select_workbench(&entries, &primary_path) {
        let path = Path::new(&entry.path);
        if !path.is_dir() {
            continue;
        }
        let base = base_of(primary, &name)?;
        let dirty = !crate::git::is_clean_ignoring_ralphy(path)?;
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
    use crate::git::git;
    use std::path::PathBuf;

    fn entry(path: &str) -> Entry {
        Entry {
            path: path.to_string(),
            ..Entry::default()
        }
    }

    #[test]
    fn parse_porcelain_reads_blank_line_separated_records() {
        let text = "worktree C:/r\nHEAD abc\nbranch refs/heads/main\n\n\
worktree C:/r/.ralphy/worktrees/wt-a\nHEAD abc\nbranch refs/heads/wt-a\n\n\
worktree C:/r/other\nHEAD abc\ndetached\n\n\
worktree C:/r/.ralphy/worktrees/gone\nHEAD abc\nbranch refs/heads/gone\n\
prunable gitdir file points to non-existent location\n\n";
        let entries = parse_porcelain(text);
        assert_eq!(entries.len(), 4, "four records: {entries:?}");
        assert_eq!(entries[0].path, "C:/r");
        assert_eq!(entries[0].branch, "main");
        assert_eq!(entries[1].branch, "wt-a");
        assert!(entries[2].detached && entries[2].branch.is_empty());
        assert!(!entries[2].prunable);
        assert!(entries[3].prunable);
        assert_eq!(entries[3].branch, "gone");

        // A last record without its closing blank line, and CRLF, both close.
        let tail = parse_porcelain("worktree C:/r\r\nHEAD abc\r\n\r\nworktree C:/r/x\r\nHEAD abc");
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[1].path, "C:/r/x");
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

    fn add_worktree(root: &Path, name: &str, path: &str) {
        git(
            root,
            &["worktree", "add", "--no-track", "-b", name, path, "main"],
        )
        .unwrap();
    }

    /// ONE fixture for the whole git-backed leg (the suite is spawn-bound):
    /// `wt-a` dirty with a recorded base, `wt-b` clean with none, `wt-gone`
    /// locked and then deleted from disk, and a worktree elsewhere that must
    /// not be reported.
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
        add_worktree(&root, "wt-a", &format!("{WORKTREES_DIR}/wt-a"));
        git(&root, &["config", "branch.wt-a.base", "main"]).unwrap();
        add_worktree(&root, "wt-b", &format!("{WORKTREES_DIR}/wt-b"));
        add_worktree(&root, "elsewhere", &elsewhere.to_string_lossy());
        // A locked worktree whose directory is gone: git still lists it, never
        // as `prunable`, and `git status` inside it would fail the listing.
        add_worktree(&root, "wt-gone", &format!("{WORKTREES_DIR}/wt-gone"));
        git(
            &root,
            &["worktree", "lock", &format!("{WORKTREES_DIR}/wt-gone")],
        )
        .unwrap();
        std::fs::remove_dir_all(root.join(WORKTREES_DIR).join("wt-gone")).unwrap();
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

        // From inside a worktree git lists the same trees, main tree first.
        assert_eq!(list(&wt_a_path).unwrap(), listing);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }
}
