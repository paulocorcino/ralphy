//! Working-tree operations: the acts that move a path between the change set's
//! two sides, the one that records them, and the one that throws a path's
//! working-tree content away — [`stage`], [`unstage`], [`commit`], [`discard`].
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

use crate::changes::{changes, ChangeStatus};
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

/// What a [`discard`] did — or why it refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscardOutcome {
    Discarded {
        restored: usize,
        deleted: usize,
    },
    NoPaths,
    NotInChangeSet {
        path: String,
    },
    /// An unresolved merge conflict: `restore --worktree` exits 1 with
    /// `path '<p>' is unmerged`, so this is a refusal by VALUE rather than
    /// git's prose relayed out.
    Conflicted {
        path: String,
    },
    /// A change-set entry with NO working-tree side — a staged deletion is the
    /// reachable one: the path is in neither the index nor the working tree, so
    /// `restore --worktree` exits 1 with `pathspec … did not match`.
    NothingInTheWorkingTree {
        path: String,
    },
}

impl DiscardOutcome {
    /// Prose for a refusal, `None` when there was none.
    pub fn reason(&self) -> Option<String> {
        Some(match self {
            DiscardOutcome::Discarded { .. } => return None,
            DiscardOutcome::NoPaths => "cannot discard: no paths were given".to_string(),
            DiscardOutcome::NotInChangeSet { path } => {
                format!("cannot discard: {path} is not in the change set")
            }
            DiscardOutcome::Conflicted { path } => {
                format!(
                    "cannot discard: {path} has an unresolved merge conflict — resolve it first"
                )
            }
            DiscardOutcome::NothingInTheWorkingTree { path } => {
                format!("cannot discard: {path} has no working-tree change — unstage it instead")
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

/// Throw away `paths`' working-tree changes — the one irreversible act here.
///
/// Two cases with different recoverability, so they run as two git shapes:
/// a TRACKED path's working tree is restored from the INDEX
/// (`restore --worktree`), which is HEAD when nothing is staged — so a staged
/// change is never silently thrown away by discarding the same path's
/// working-tree edit. An UNTRACKED entry is DELETED (`clean --force -d`), and
/// no commit and no reflog can bring it back; `-d` is what makes an untracked
/// DIRECTORY discardable, because the change set reports one as a single entry
/// (`newdir/`).
///
/// Cross-path invariant: the whole list is partitioned BEFORE either write, so
/// no path that returns a refusal has run a git write. Within a mixed batch the
/// restores run FIRST and the deletions LAST — the unrecoverable act happens
/// last, so a failure in the recoverable half never leaves a file deleted for
/// nothing.
///
/// A rename's ORIGINAL path is NOT discardable: it names no working-tree
/// content, the same asymmetry [`stage`] documents.
///
/// Being IN the change set is not enough to be restorable, and the two rows
/// that are not are refused as VALUES rather than relayed as git's prose
/// (measured, both exit 1): an unresolved conflict — `path '<p>' is unmerged`
/// — and an entry with no working-tree side at all, i.e. a staged deletion —
/// `pathspec '<p>' did not match any file(s) known to git`.
pub fn discard(repo: &Path, paths: &[String]) -> Result<DiscardOutcome> {
    if paths.is_empty() {
        return Ok(DiscardOutcome::NoPaths);
    }
    let mut known: HashSet<String> = HashSet::new();
    let mut loose: HashSet<String> = HashSet::new();
    let mut conflicted: HashSet<String> = HashSet::new();
    let mut no_worktree_side: HashSet<String> = HashSet::new();
    for change in changes(repo)? {
        if change.status == ChangeStatus::Conflicted {
            conflicted.insert(change.path.clone());
        } else if change.status == ChangeStatus::Untracked {
            loose.insert(change.path.clone());
        } else if change.worktree_status.is_none() {
            no_worktree_side.insert(change.path.clone());
        }
        known.insert(change.path);
    }
    let mut tracked: Vec<String> = Vec::new();
    let mut untracked: Vec<String> = Vec::new();
    for path in paths {
        if !known.contains(path) {
            return Ok(DiscardOutcome::NotInChangeSet { path: path.clone() });
        }
        if conflicted.contains(path) {
            return Ok(DiscardOutcome::Conflicted { path: path.clone() });
        }
        if no_worktree_side.contains(path) {
            return Ok(DiscardOutcome::NothingInTheWorkingTree { path: path.clone() });
        }
        if loose.contains(path) {
            untracked.push(path.clone());
        } else {
            tracked.push(path.clone());
        }
    }
    if !tracked.is_empty() {
        run_on_paths(
            repo,
            &["--literal-pathspecs", "restore", "--worktree", "--"],
            &tracked,
            "discarding",
        )?;
    }
    if !untracked.is_empty() {
        run_on_paths(
            repo,
            &["--literal-pathspecs", "clean", "--force", "-d", "--"],
            &untracked,
            "deleting",
        )?;
    }
    Ok(DiscardOutcome::Discarded {
        restored: tracked.len(),
        deleted: untracked.len(),
    })
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
