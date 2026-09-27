//! `ralphy changes list|stage|unstage|commit|discard` — the working-tree change
//! set of a repo and the four acts that move paths through it. Every primitive
//! delegates
//! to an already-public [`ralphy_core::changes`] / [`ralphy_core::worktree`]
//! function; this module is only the guard + clap surface and the output shapes.
//!
//! `list` is read-only and never consults `.ralphy/run.lock` (see
//! `mutate::branch`'s `List` arm). `stage`, `unstage`, `commit` and `discard`
//! inspect it and
//! refuse under [`crate::runlock::LockState::HeldAlive`] before any git WRITE
//! (ADR-0036 §6). Precisely: the one git call the guard does not precede is the
//! read-only `rev-parse --show-toplevel` that LOCATES the lock — it has to run
//! first, and it mirrors `sync.rs`'s ordering.
//!
//! No write takes `--format`: a refusal reaches the workbench only through the
//! non-zero-exit message path (the daemon's Mutate branch collapses a successful
//! exit to `{"status":"ok"}` and discards stdout), so the outcome's own prose is
//! carried on stderr by exiting non-zero.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use ralphy_core::worktree::{CommitOutcome, DiscardOutcome, StageOutcome, UnstageOutcome};

use crate::runlock;
use crate::runlock::guard_run_lock;

#[derive(Subcommand)]
pub(crate) enum ChangesCommand {
    /// List the changes in the repo that are not committed. Changes nothing.
    List(ChangesListArgs),
    /// Stage these files for the next commit. Refused while a run is working in
    /// the repo.
    Stage(ChangesPathArgs),
    /// Unstage these files. Refused while a run is working in the repo.
    Unstage(ChangesPathArgs),
    /// Commit the staged files. Refused while a run is working in the repo.
    Commit(ChangesCommitArgs),
    /// Throw away the changes in these files. Refused while a run is working in
    /// the repo.
    Discard(ChangesPathArgs),
}

#[derive(Args)]
pub(crate) struct ChangesListArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// Output: text (the default) or `json`.
    #[arg(long)]
    pub(crate) format: Option<String>,
}

#[derive(Args)]
pub(crate) struct ChangesPathArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// A file path, relative to the repo (repeat for more). It must be one of
    /// the changed files; patterns are not accepted.
    #[arg(long)]
    pub(crate) path: Vec<String>,
}

#[derive(Args)]
pub(crate) struct ChangesCommitArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// The commit message. Write it as `--message=<msg>`, so that a message
    /// that starts with `-` is not read as an option.
    #[arg(long)]
    pub(crate) message: String,
}

/// `ralphy changes list|stage|unstage|commit|discard`.
pub(crate) fn changes(cmd: ChangesCommand) -> anyhow::Result<()> {
    match cmd {
        ChangesCommand::List(args) => changes_list(args),
        ChangesCommand::Stage(args) => changes_stage(args),
        ChangesCommand::Unstage(args) => changes_unstage(args),
        ChangesCommand::Commit(args) => changes_commit(args),
        ChangesCommand::Discard(args) => changes_discard(args),
    }
}

/// `ralphy changes list [--format json]`. Read-only: no run-lock guard.
fn changes_list(args: ChangesListArgs) -> anyhow::Result<()> {
    let repo_root = ralphy_core::git::resolve_toplevel(&args.repo)?;
    let list = ralphy_core::changes::changes(&repo_root)?;

    if args.format.as_deref() == Some("json") {
        let out = serde_json::json!({ "changes": list });
        println!("{out}");
    } else {
        for c in &list {
            match &c.original_path {
                Some(from) => println!("renamed {from} -> {}", c.path),
                None => println!("{} {}", status_word(c.status), c.path),
            }
        }
    }
    Ok(())
}

/// Resolve the toplevel and refuse under a live run lock — in that order, and
/// before any `ralphy_core::worktree` call, so a guarded verb reaches no git.
fn guarded_root(repo: &Path, verb: &str) -> anyhow::Result<PathBuf> {
    let repo_root = ralphy_core::git::resolve_toplevel(repo)?;
    let ws = ralphy_core::Workspace::new(&repo_root);
    guard_run_lock(&ws, verb, runlock::pid_is_alive)?;
    Ok(repo_root)
}

/// `ralphy changes stage --path <p> [--path <p>…]`.
fn changes_stage(args: ChangesPathArgs) -> anyhow::Result<()> {
    let repo_root = guarded_root(&args.repo, "changes stage")?;
    match ralphy_core::worktree::stage(&repo_root, &args.path)? {
        StageOutcome::Staged { paths } => println!("Staged {paths} path(s)."),
        refused => anyhow::bail!(
            "{}",
            refused.reason().unwrap_or_else(|| format!("{refused:?}"))
        ),
    }
    Ok(())
}

/// `ralphy changes unstage --path <p> [--path <p>…]`.
fn changes_unstage(args: ChangesPathArgs) -> anyhow::Result<()> {
    let repo_root = guarded_root(&args.repo, "changes unstage")?;
    match ralphy_core::worktree::unstage(&repo_root, &args.path)? {
        UnstageOutcome::Unstaged { paths } => println!("Unstaged {paths} path(s)."),
        refused => anyhow::bail!(
            "{}",
            refused.reason().unwrap_or_else(|| format!("{refused:?}"))
        ),
    }
    Ok(())
}

/// `ralphy changes commit --message=<msg>`.
fn changes_commit(args: ChangesCommitArgs) -> anyhow::Result<()> {
    let repo_root = guarded_root(&args.repo, "changes commit")?;
    match ralphy_core::worktree::commit(&repo_root, &args.message)? {
        CommitOutcome::Committed { sha } => println!("Committed {sha}."),
        refused => anyhow::bail!(
            "{}",
            refused.reason().unwrap_or_else(|| format!("{refused:?}"))
        ),
    }
    Ok(())
}

/// `ralphy changes discard --path <p> [--path <p>…]`.
fn changes_discard(args: ChangesPathArgs) -> anyhow::Result<()> {
    let repo_root = guarded_root(&args.repo, "changes discard")?;
    match ralphy_core::worktree::discard(&repo_root, &args.path)? {
        DiscardOutcome::Discarded { restored, deleted } => {
            println!("Discarded {restored} path(s), deleted {deleted} untracked path(s).")
        }
        refused => anyhow::bail!(
            "{}",
            refused.reason().unwrap_or_else(|| format!("{refused:?}"))
        ),
    }
    Ok(())
}

fn status_word(status: ralphy_core::ChangeStatus) -> &'static str {
    use ralphy_core::ChangeStatus::*;
    match status {
        Modified => "modified",
        Added => "added",
        Deleted => "deleted",
        Renamed => "renamed",
        Untracked => "untracked",
        Conflicted => "conflicted",
    }
}
