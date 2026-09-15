//! Checkouts: the git worktrees Ralphy created under `.ralphy/worktrees/<name>`
//! (ADR-0063 §1–§2). [`list`] reports them with their branch, recorded base
//! and dirty flag, from any starting directory (the primary tree or one of
//! the worktrees): git lists a repository's worktrees the same from every
//! tree of it, main tree first. [`add`] creates one on a new branch and
//! records the branch it was cut from as `branch.<name>.base`. [`remove`]
//! takes one away behind its gates — locked, dirty, no `--force`, `branch -d`
//! never `-D` — each refusal a [`RemoveError`].
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

/// One record of `git worktree list --porcelain`, before the filter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Entry {
    pub(crate) path: String,
    pub(crate) branch: String,
    pub(crate) detached: bool,
    pub(crate) prunable: bool,
    /// Git's own `locked` line (git ≥ 2.31), with or without a reason.
    pub(crate) locked: bool,
}

/// Parse `git worktree list --porcelain`: one attribute per line, a blank line
/// closes the record. The newline form and not `-z`: `-z` needs git 2.36 and
/// the operator's WSL peer ships Ubuntu 22.04's 2.34, while a path with a
/// newline in it is one the workbench never creates. `HEAD` and `bare` are
/// read and ignored.
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
        } else if line == "locked" || line.starts_with("locked ") {
            entry.locked = true;
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
    let entries = entries(start)?;
    let primary_path = entries[0].path.clone();
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

/// Every worktree git knows for the repository containing `start`, main tree
/// first. Never empty: git always lists at least the tree it was asked from.
pub(crate) fn entries(start: &Path) -> Result<Vec<Entry>> {
    let out = raw(start, &["worktree", "list", "--porcelain"])?;
    if !out.status.success() {
        bail!(
            "`git worktree list --porcelain` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let entries = parse_porcelain(&String::from_utf8_lossy(&out.stdout));
    if entries.is_empty() {
        bail!("git listed no worktree for {}", start.display());
    }
    Ok(entries)
}

/// The primary (main) tree of the repository containing `start`, in git's own
/// spelling — the same record [`list`] reports as `primary`.
pub fn primary(start: &Path) -> Result<PathBuf> {
    let entries = entries(start)?;
    Ok(PathBuf::from(&entries[0].path))
}

/// Create the workbench worktree `<primary>/.ralphy/worktrees/<name>` on a new
/// branch `<name>` cut from `base` (the primary's current branch when `None`),
/// and record that base as `branch.<name>.base` in the primary's config
/// (ADR-0063 §2).
///
/// Five refusals, each before anything is written: a name git would not take
/// as a branch, a name with a path separator, a branch checked out in some
/// tree, a branch that already exists, and a leftover directory at the
/// worktree's location. Git creates the branch before it validates the
/// target, so a failing worktree spawn deletes the branch it left behind, and
/// a failing config write rolls the worktree and the branch back: on every
/// return path either both exist or neither does.
pub fn add(start: &Path, name: &str, base: Option<&str>) -> Result<Checkout> {
    let entries = entries(start)?;
    let primary_path = entries[0].path.clone();
    let primary = Path::new(&primary_path);
    if name.is_empty()
        || name.starts_with('-')
        || !raw(primary, &["check-ref-format", "--branch", name])?
            .status
            .success()
    {
        bail!("invalid worktree name '{name}': not a valid branch name");
    }
    if name.contains(['/', '\\']) {
        bail!("invalid worktree name '{name}': must be a single path segment");
    }
    if let Some(entry) = entries.iter().find(|e| !e.detached && e.branch == name) {
        bail!("branch '{name}' is checked out at {}", entry.path);
    }
    let ref_name = format!("refs/heads/{name}");
    let probe = raw(primary, &["show-ref", "--verify", "--quiet", &ref_name])?;
    if probe.status.success() {
        bail!("branch '{name}' already exists");
    }
    if probe.status.code() != Some(1) {
        bail!(
            "`git show-ref --verify {ref_name}` failed: {}",
            String::from_utf8_lossy(&probe.stderr).trim()
        );
    }
    let rel = format!("{WORKTREES_DIR}/{name}");
    if primary.join(&rel).exists() {
        bail!("a directory already exists at {rel}: remove it first");
    }
    let base = match base.map(str::trim).filter(|b| !b.is_empty()) {
        Some("HEAD") => bail!("base 'HEAD' is not a branch: pass a branch name"),
        Some(b) => b.to_string(),
        None => {
            let current = crate::git::current_branch(primary)?;
            if current == "HEAD" {
                bail!("the primary tree is detached: pass --base <ref>");
            }
            current
        }
    };
    std::fs::create_dir_all(primary.join(WORKTREES_DIR))
        .with_context(|| format!("creating {}/{WORKTREES_DIR}", primary.display()))?;
    if let Err(e) = git(
        primary,
        &["worktree", "add", "--no-track", "-b", name, &rel, &base],
    ) {
        // `-b` lands before git validates the target: delete the branch it may
        // have left, so a retry is not refused with `already exists`.
        delete_branch_best_effort(primary, name);
        return Err(e.context(format!("creating worktree '{name}'")));
    }
    let key = format!("branch.{name}.base");
    if let Err(e) = git(primary, &["config", "--local", &key, &base]) {
        // Roll back so nothing half-made survives.
        if let Err(rm) = git(primary, &["worktree", "remove", "--force", &rel]) {
            tracing::warn!(error = %rm, path = %rel, "could not roll back the worktree");
        }
        delete_branch_best_effort(primary, name);
        return Err(e.context(format!("recording {key} (the worktree was rolled back)")));
    }
    Ok(Checkout {
        name: name.to_string(),
        path: format!("{primary_path}/{WORKTREES_DIR}/{name}"),
        branch: name.to_string(),
        base,
        dirty: false,
    })
}

/// Why [`remove`] refused, one variant per gate (ADR-0063 §1). Returned bare —
/// never under a `.context()` — so the CLI's stderr is the one line the daemon
/// relays and the picker shows verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveError {
    NotFound {
        name: String,
    },
    Locked {
        name: String,
    },
    Dirty {
        name: String,
    },
    RemoveFailed {
        name: String,
        detail: String,
    },
    /// The directory is gone; the branch was not `-d`-deletable and stays.
    BranchKept {
        name: String,
        why: String,
    },
}

impl std::fmt::Display for RemoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { name } => write!(f, "no workbench worktree named '{name}'"),
            Self::Locked { name } => write!(f, "worktree '{name}' is locked: unlock it first"),
            Self::Dirty { name } => write!(
                f,
                "worktree '{name}' has uncommitted changes: commit or discard them first"
            ),
            Self::RemoveFailed { name, detail } => {
                write!(f, "removing worktree '{name}' failed: {detail}")
            }
            Self::BranchKept { name, why } => {
                write!(f, "removed worktree '{name}'; branch '{name}' kept: {why}")
            }
        }
    }
}

impl std::error::Error for RemoveError {}

/// Remove the workbench worktree `<name>` and its branch, behind ADR-0063 §1's
/// gates in this order: locked (git's own word), dirty, then `worktree remove`
/// with no `--force`, then `branch -d` — never `-D`. Each gate is its own
/// [`RemoveError`]; when `-d` refuses, the directory is already gone and the
/// branch is kept, and the error says why. Infrastructure failures (a git
/// spawn) keep their anyhow context.
pub fn remove(start: &Path, name: &str) -> Result<()> {
    let entries = entries(start)?;
    let primary_path = entries[0].path.clone();
    let primary = Path::new(&primary_path);
    let not_found = || RemoveError::NotFound {
        name: name.to_string(),
    };
    let Some((_, entry)) = select_workbench(&entries, &primary_path)
        .into_iter()
        .find(|(n, _)| n == name)
    else {
        return Err(not_found().into());
    };
    if entry.locked {
        return Err(RemoveError::Locked {
            name: name.to_string(),
        }
        .into());
    }
    let path = Path::new(&entry.path);
    if !path.is_dir() {
        return Err(not_found().into());
    }
    if !crate::git::is_clean_ignoring_ralphy(path)? {
        return Err(RemoveError::Dirty {
            name: name.to_string(),
        }
        .into());
    }
    let rel = format!("{WORKTREES_DIR}/{name}");
    let base = base_of(primary, name)?;
    let out = raw(primary, &["worktree", "remove", &rel])?;
    if !out.status.success() {
        return Err(RemoveError::RemoveFailed {
            name: name.to_string(),
            detail: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        }
        .into());
    }
    let out = raw(primary, &["branch", "-d", name])?;
    if out.status.success() {
        return Ok(());
    }
    let git_stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(RemoveError::BranchKept {
        name: name.to_string(),
        why: branch_kept_why(primary, name, &base, &git_stderr)?,
    }
    .into())
}

/// Phrase why `branch -d <name>` refused. `-d` checks merge against the
/// primary's HEAD, not the recorded base, so a branch with nothing of its own
/// can still be refused when the base moved on; one `merge-base --is-ancestor`
/// probe tells the two apart, and the message never claims commits that do not
/// exist.
fn branch_kept_why(primary: &Path, name: &str, base: &str, git_stderr: &str) -> Result<String> {
    let head_form =
        |detail: &str| format!("it is not merged into the primary's HEAD (git: {detail})");
    if base.is_empty() {
        return Ok(head_form(git_stderr));
    }
    let probe = raw(
        primary,
        &[
            "merge-base",
            "--is-ancestor",
            &format!("refs/heads/{name}"),
            base,
        ],
    )?;
    Ok(match probe.status.code() {
        Some(1) => format!("it has commits not on {base}"),
        Some(0) => head_form(git_stderr),
        _ => head_form(String::from_utf8_lossy(&probe.stderr).trim()),
    })
}

/// Delete `<name>` if it exists, `-D` because `-d`'s merged check runs against
/// the primary's HEAD, not the base the branch was cut from — and at every
/// call site the branch has no commit of its own, so nothing is lost.
fn delete_branch_best_effort(primary: &Path, name: &str) {
    let ref_name = format!("refs/heads/{name}");
    match raw(primary, &["show-ref", "--verify", "--quiet", &ref_name]) {
        Ok(out) if out.status.success() => {
            if let Err(rm) = git(primary, &["branch", "-D", name]) {
                tracing::warn!(error = %rm, branch = name, "could not roll back the branch");
            }
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, branch = name, "could not probe the branch"),
    }
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
prunable gitdir file points to non-existent location\n\n\
worktree C:/r/.ralphy/worktrees/held\nHEAD abc\nbranch refs/heads/held\nlocked operator says so\n\n\
worktree C:/r/.ralphy/worktrees/bare-lock\nHEAD abc\nbranch refs/heads/bare-lock\nlocked\n\n";
        let entries = parse_porcelain(text);
        assert_eq!(entries.len(), 6, "six records: {entries:?}");
        assert!(
            entries[4].locked && entries[5].locked,
            "locked with and without a reason"
        );
        assert!(!entries[1].locked, "an unlocked record stays unlocked");
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

    /// ONE fixture: `wt-c` from the default base, `wt-d` from an explicit
    /// one, then the four refusals against the same repo, each checked to
    /// have left nothing behind.
    #[test]
    fn add_creates_a_worktree_and_refuses_each_bad_input() {
        let root = tmp("add");
        git(&root, &["init", "-q", "-b", "main"]).unwrap();
        configure(&root);
        commit_file(&root, "README.md", "hello\n", "init");
        commit_file(&root, ".gitignore", ".ralphy/\n", "ignore the run dir");
        git(&root, &["branch", "taken"]).unwrap();

        let c = add(&root, "wt-c", None).unwrap();
        assert_eq!(c.name, "wt-c");
        assert_eq!(c.branch, "wt-c");
        assert_eq!(c.base, "main");
        assert!(!c.dirty);
        assert!(
            c.path.ends_with("/.ralphy/worktrees/wt-c"),
            "path in git's spelling: {}",
            c.path
        );
        let wt_c_dir = root.join(WORKTREES_DIR).join("wt-c");
        assert!(wt_c_dir.is_dir());
        assert_eq!(git(&root, &["config", "branch.wt-c.base"]).unwrap(), "main");
        assert_eq!(
            git(&wt_c_dir, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
            "wt-c"
        );

        let d = add(&root, "wt-d", Some("taken")).unwrap();
        assert_eq!(d.base, "taken");

        // A leftover directory at the location (no git registration): the
        // fifth gate, before git could leave an orphaned branch behind.
        std::fs::create_dir_all(root.join(WORKTREES_DIR).join("left")).unwrap();
        std::fs::write(root.join(WORKTREES_DIR).join("left").join("x"), "x").unwrap();
        let refusals = [
            ("a..b", "not a valid branch name"),
            ("a/b", "must be a single path segment"),
            ("taken", "already exists"),
            ("main", "is checked out at"),
            ("left", "a directory already exists at"),
        ];
        for (name, needle) in refusals {
            let err = add(&root, name, None).unwrap_err().to_string();
            assert!(err.contains(needle), "{name}: {err}");
            assert!(
                name == "left" || !root.join(WORKTREES_DIR).join(name).exists(),
                "{name}: nothing written on refusal"
            );
        }
        let err = add(&root, "wt-h", Some("HEAD")).unwrap_err().to_string();
        assert!(err.contains("is not a branch"), "{err}");
        for name in ["a..b", "a/b", "left", "wt-h"] {
            let probe = raw(
                &root,
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{name}"),
                ],
            )
            .unwrap();
            assert!(!probe.status.success(), "{name}: no branch was created");
        }

        let listing = list(&root).unwrap();
        let names: Vec<&str> = listing.worktrees.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, vec!["wt-c", "wt-d"], "listing: {listing:?}");
        assert_eq!(listing.worktrees[0].base, "main");
        assert_eq!(listing.worktrees[1].base, "taken");

        let _ = std::fs::remove_dir_all(&root);
    }

    fn branch_exists(root: &Path, name: &str) -> bool {
        raw(
            root,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ],
        )
        .unwrap()
        .status
        .success()
    }

    fn remove_error(root: &Path, name: &str) -> (RemoveError, String) {
        let err = remove(root, name).unwrap_err();
        let text = err.to_string();
        let domain = err
            .downcast_ref::<RemoveError>()
            .unwrap_or_else(|| panic!("{name}: not a RemoveError: {err:?}"))
            .clone();
        (domain, text)
    }

    /// ONE fixture, the gates in ADR-0063 §1's order: `wt-lock` is locked AND
    /// dirty and answers `Locked` (the ordering oracle), then unlocked answers
    /// `Dirty`, then cleaned answers `Ok` with directory and branch gone;
    /// `wt-keep` carries a commit beyond its base and answers `BranchKept` with
    /// the directory gone and the branch present; an unknown name and the
    /// primary itself answer `NotFound`.
    #[test]
    fn remove_applies_the_gates_in_order() {
        let root = tmp("remove");
        git(&root, &["init", "-q", "-b", "main"]).unwrap();
        configure(&root);
        commit_file(&root, "README.md", "hello\n", "init");
        commit_file(&root, ".gitignore", ".ralphy/\n", "ignore the run dir");

        add(&root, "wt-lock", None).unwrap();
        let wt_lock = root.join(WORKTREES_DIR).join("wt-lock");
        let rel_lock = format!("{WORKTREES_DIR}/wt-lock");
        git(&root, &["worktree", "lock", "--reason", "held", &rel_lock]).unwrap();
        std::fs::write(wt_lock.join("scratch.txt"), "dirty\n").unwrap();

        // (1) locked AND dirty: the lock gate answers first.
        let (err, text) = remove_error(&root, "wt-lock");
        assert_eq!(
            err,
            RemoveError::Locked {
                name: "wt-lock".into()
            }
        );
        assert!(text.contains("is locked"), "{text}");
        assert!(wt_lock.is_dir(), "a refusal keeps the directory");

        // (2) unlocked but dirty.
        git(&root, &["worktree", "unlock", &rel_lock]).unwrap();
        let (err, text) = remove_error(&root, "wt-lock");
        assert_eq!(
            err,
            RemoveError::Dirty {
                name: "wt-lock".into()
            }
        );
        assert!(text.contains("has uncommitted changes"), "{text}");
        assert!(wt_lock.is_dir(), "a refusal keeps the directory");

        // (3) clean: directory and branch go.
        std::fs::remove_file(wt_lock.join("scratch.txt")).unwrap();
        remove(&root, "wt-lock").unwrap();
        assert!(!wt_lock.exists(), "the directory is gone");
        assert!(!branch_exists(&root, "wt-lock"), "the branch is deleted");
        assert_eq!(
            base_of(&root, "wt-lock").unwrap(),
            "",
            "no branch.wt-lock.base survives the branch"
        );

        // (4) a commit beyond the base: the directory goes, the branch stays.
        add(&root, "wt-keep", None).unwrap();
        let wt_keep = root.join(WORKTREES_DIR).join("wt-keep");
        configure(&wt_keep);
        commit_file(&wt_keep, "feature.txt", "x\n", "beyond");
        let (err, text) = remove_error(&root, "wt-keep");
        match &err {
            RemoveError::BranchKept { name, why } => {
                assert_eq!(name, "wt-keep");
                assert!(why.contains("not on main"), "why: {why}");
            }
            other => panic!("expected BranchKept, got {other:?}"),
        }
        assert!(text.contains("branch 'wt-keep' kept"), "{text}");
        assert!(!wt_keep.exists(), "the directory is gone");
        assert!(branch_exists(&root, "wt-keep"), "the branch is kept");

        // (5) unknown names, the primary included.
        for name in ["nope", "primary"] {
            let (err, _) = remove_error(&root, name);
            assert_eq!(err, RemoveError::NotFound { name: name.into() });
        }

        // (6) nothing left to list.
        assert!(list(&root).unwrap().worktrees.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }
}
