//! Checkouts: the git worktrees Ralphy created under `.ralphy/worktrees/<name>`
//! (ADR-0063 §1–§2). [`list`] reports them with their branch, recorded base
//! and dirty flag, from any starting directory (the primary tree or one of
//! the worktrees): git lists a repository's worktrees the same from every
//! tree of it, main tree first. [`add`] creates one on a new branch and
//! records the branch it was cut from as `branch.<name>.base`. [`remove`]
//! takes one away behind its gates — locked, dirty, no `--force`, `branch -d`
//! never `-D` — each refusal a [`RemoveError`]. [`carry_over`] gives a new
//! worktree the gitignored paths `settings.json` names (warn-only).
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

mod carry;
pub use carry::{carry_over, unlink_shares};

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
    crate::git::validate_branch_name(primary, name)
        .with_context(|| format!("invalid worktree name '{name}': not a valid branch name"))?;
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
        // The last positional of `git worktree add`: a `-`-leading base would
        // be read as an option (`--detach`, `--lock`), so it must resolve to a
        // commit BEFORE git sees it (audit F11).
        Some(b) => {
            crate::git::validate_commitish(primary, b)
                .with_context(|| format!("invalid base '{b}'"))?;
            b.to_string()
        }
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
    // The shares first, by hand: `git worktree remove` follows a junction
    // into the primary's own directory and deletes it (measured on Windows,
    // 2026-09-16). A link that will not go is a refusal — never a removal
    // that might descend.
    let stuck = unlink_shares(path);
    if let Some(first) = stuck.first() {
        return Err(RemoveError::RemoveFailed {
            name: name.to_string(),
            detail: format!("could not unlink a shared directory first: {first}"),
        }
        .into());
    }
    let out = raw(primary, &["worktree", "remove", &rel])?;
    if !out.status.success() {
        return Err(RemoveError::RemoveFailed {
            name: name.to_string(),
            detail: one_line(&out.stderr),
        }
        .into());
    }
    let out = raw(primary, &["branch", "-d", name])?;
    if out.status.success() {
        return Ok(());
    }
    Err(RemoveError::BranchKept {
        name: name.to_string(),
        why: branch_kept_why(primary, name, &base, &one_line(&out.stderr))?,
    }
    .into())
}

/// Git's stderr as ONE line (`; `-joined): a `RemoveError` is relayed and
/// rendered verbatim, and git's `hint:` lines would break that.
fn one_line(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Phrase why `branch -d <name>` refused. `-d` checks merge against the
/// primary's HEAD, not the recorded base, so a branch with nothing of its own
/// can still be refused when the base moved on; one `merge-base --is-ancestor`
/// probe tells the two apart, and the message never claims commits that do not
/// exist. A refusal that is not about merging (the branch is checked out in
/// another tree, or is gone) relays git's own words.
fn branch_kept_why(primary: &Path, name: &str, base: &str, git_stderr: &str) -> Result<String> {
    if !git_stderr.contains("not fully merged") {
        return Ok(format!("git: {git_stderr}"));
    }
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
        _ => head_form(&one_line(&probe.stderr)),
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
mod tests;
