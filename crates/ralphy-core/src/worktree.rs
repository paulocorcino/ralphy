//! Working-tree operations: the acts that move a path between the change set's
//! two sides and the one that records them — [`stage`], [`unstage`], [`commit`].
//!
//! Every refusal here is a VALUE, not an `Err`: "nothing is staged" and "the
//! message is empty" are answers to a reasonable question, and the caller
//! matches on the variant rather than grepping git's stderr. An `Err` is
//! reserved for a real failure (git missing, an unconfigured `user.email`, a
//! repo that cannot be read).
//!
//! Scope: this module holds the working tree only. The branch's relation to its
//! upstream and the two acts that change it — `fetch` and a fast-forward-only
//! `pull` — live in [`crate::sync`]; they do not belong here just because they
//! are also git.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{bail, Result};

use crate::changes::changes;
use crate::git::{git, raw, raw_env};

/// What a [`stage`] did — or why it refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageOutcome {
    Staged { paths: usize },
    NoPaths,
    NotInChangeSet { path: String },
}

impl StageOutcome {
    /// Prose for a refusal, `None` when there was none. Composed here rather
    /// than relayed from git's stderr — see [`crate::sync::FetchOutcome::reason`].
    pub fn reason(&self) -> Option<String> {
        Some(match self {
            StageOutcome::Staged { .. } => return None,
            StageOutcome::NoPaths => "cannot stage: no paths were given".to_string(),
            StageOutcome::NotInChangeSet { path } => {
                format!("cannot stage: {path} is not in the change set")
            }
        })
    }
}

/// What an [`unstage`] did — or why it refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnstageOutcome {
    Unstaged { paths: usize },
    NoPaths,
    NotInChangeSet { path: String },
}

impl UnstageOutcome {
    /// Prose for a refusal, `None` when there was none.
    pub fn reason(&self) -> Option<String> {
        Some(match self {
            UnstageOutcome::Unstaged { .. } => return None,
            UnstageOutcome::NoPaths => "cannot unstage: no paths were given".to_string(),
            UnstageOutcome::NotInChangeSet { path } => {
                format!("cannot unstage: {path} is not in the change set")
            }
        })
    }
}

/// What a [`commit`] did — or why it refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    Committed { sha: String },
    NothingStaged,
    EmptyMessage,
}

impl CommitOutcome {
    /// Prose for a refusal, `None` when the commit was recorded.
    pub fn reason(&self) -> Option<String> {
        Some(match self {
            CommitOutcome::Committed { .. } => return None,
            CommitOutcome::NothingStaged => {
                "cannot commit: nothing is staged — stage a file first".to_string()
            }
            CommitOutcome::EmptyMessage => "cannot commit: the message is empty".to_string(),
        })
    }
}

/// Add `paths` to the index.
///
/// `repo` must be the git TOPLEVEL, as [`crate::changes::changes`] requires;
/// callers resolve with [`crate::git::resolve_toplevel`].
///
/// A path absent from the change set is refused as a VALUE rather than relayed
/// as git's "pathspec did not match": the change set is the single definition of
/// what the panel can act on, and re-using it keeps a caller from having to read
/// git's prose.
///
/// A rename's ORIGINAL path is deliberately NOT stageable. After `git mv a b`
/// the old path exists in neither the index nor the working tree, so `git add a`
/// is fatal — and `git add` aborts the WHOLE invocation on one unmatched
/// pathspec, so accepting it would make a group action stage nothing at all.
/// [`unstage`] is the direction that needs it.
pub fn stage(repo: &Path, paths: &[String]) -> Result<StageOutcome> {
    if paths.is_empty() {
        return Ok(StageOutcome::NoPaths);
    }
    if let Some(path) = first_unknown(repo, paths, Rename::NewPathOnly)? {
        return Ok(StageOutcome::NotInChangeSet { path });
    }
    run_on_paths(
        repo,
        &["--literal-pathspecs", "add", "--"],
        paths,
        "staging",
    )?;
    Ok(StageOutcome::Staged { paths: paths.len() })
}

/// Remove `paths` from the index, leaving the working tree untouched.
///
/// `git restore --staged` cannot resolve an unborn HEAD, so a repo that has
/// never committed is unstaged with `git rm --cached` instead — refusing there
/// would break the first repo an operator registers. The probe is
/// [`crate::sync`]'s: `rev-parse --verify --quiet HEAD`.
///
/// A rename's ORIGINAL path IS accepted here: `git restore --staged` needs it to
/// undo the deletion half, and it is still a live index entry.
pub fn unstage(repo: &Path, paths: &[String]) -> Result<UnstageOutcome> {
    if paths.is_empty() {
        return Ok(UnstageOutcome::NoPaths);
    }
    if let Some(path) = first_unknown(repo, paths, Rename::BothPaths)? {
        return Ok(UnstageOutcome::NotInChangeSet { path });
    }
    let born = raw(repo, &["rev-parse", "--verify", "--quiet", "HEAD"])?
        .status
        .success();
    let prefix: &[&str] = if born {
        &["--literal-pathspecs", "restore", "--staged", "--"]
    } else {
        &["--literal-pathspecs", "rm", "--cached", "--quiet", "--"]
    };
    run_on_paths(repo, prefix, paths, "unstaging")?;
    Ok(UnstageOutcome::Unstaged { paths: paths.len() })
}

/// Record the staged index as a commit.
///
/// Cross-path invariant: NO path that returns a refusal has run `git commit`.
/// The order is decide-then-write — the empty-message check makes no git call at
/// all, and the nothing-staged check is a READ of the change set.
///
/// The message travels as ONE `--message=<msg>` token so a message beginning
/// with `-` is never re-read as an option.
///
/// `GIT_TERMINAL_PROMPT=0` is pinned for the same reason [`crate::sync::fetch`]
/// pins it: this call runs under a console-less daemon child, and `git commit`
/// is the first verb here that runs the repo's own hooks and may reach a GPG
/// signer. A prompt on a terminal nobody can see would hang the click forever.
/// Hooks themselves are NOT skipped — `--no-verify` would silently disable the
/// operator's own gate.
pub fn commit(repo: &Path, message: &str) -> Result<CommitOutcome> {
    if message.trim().is_empty() {
        return Ok(CommitOutcome::EmptyMessage);
    }
    if !changes(repo)?.iter().any(|c| c.index_status.is_some()) {
        return Ok(CommitOutcome::NothingStaged);
    }
    let arg = format!("--message={message}");
    let out = raw_env(
        repo,
        &["commit", "--quiet", &arg],
        &[("GIT_TERMINAL_PROMPT", "0")],
    )?;
    if !out.status.success() {
        bail!(
            "committing: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let sha = git(repo, &["rev-parse", "--short", "HEAD"])?;
    Ok(CommitOutcome::Committed { sha })
}

/// Which side(s) of a rename count as "in the change set" for one direction.
/// The two directions really do differ — see [`stage`] and [`unstage`] — and a
/// single shared answer is what let a group stage abort on `git add <old path>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rename {
    NewPathOnly,
    BothPaths,
}

/// The first of `paths` the change set does not name, `None` when every one of
/// them is in it.
fn first_unknown(repo: &Path, paths: &[String], rename: Rename) -> Result<Option<String>> {
    let mut known: HashSet<String> = HashSet::new();
    for change in changes(repo)? {
        if rename == Rename::BothPaths {
            if let Some(original) = change.original_path {
                known.insert(original);
            }
        }
        known.insert(change.path);
    }
    Ok(paths.iter().find(|p| !known.contains(*p)).cloned())
}

/// Run `git <prefix> <paths…>`, bailing with git's stderr on a non-zero exit.
fn run_on_paths(repo: &Path, prefix: &[&str], paths: &[String], doing: &str) -> Result<()> {
    let mut args: Vec<&str> = prefix.to_vec();
    args.extend(paths.iter().map(String::as_str));
    let out = raw(repo, &args)?;
    if !out.status.success() {
        bail!(
            "{doing} {} path(s): {}",
            paths.len(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
