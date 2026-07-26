//! `ralphy changes list|stage|unstage|commit` — the working-tree change set of a
//! repo and the three acts that move paths through it. Every primitive delegates
//! to an already-public [`ralphy_core::changes`] / [`ralphy_core::worktree`]
//! function; this module is only the guard + clap surface and the output shapes.
//!
//! `list` is read-only and never consults `.ralphy/run.lock` (see
//! `mutate::branch`'s `List` arm). `stage`, `unstage` and `commit` inspect it and
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
use ralphy_core::worktree::{CommitOutcome, StageOutcome, UnstageOutcome};

use crate::runlock;
use crate::runlock::guard_run_lock;

#[derive(Subcommand)]
pub(crate) enum ChangesCommand {
    /// List the repo's working-tree changes (read-only; never consults the run.lock).
    List(ChangesListArgs),
    /// Add the given paths to the index (refuses under a held run.lock).
    Stage(ChangesPathArgs),
    /// Remove the given paths from the index (refuses under a held run.lock).
    Unstage(ChangesPathArgs),
    /// Record the staged index as a commit (refuses under a held run.lock).
    Commit(ChangesCommitArgs),
}

#[derive(Args)]
pub(crate) struct ChangesListArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// Output format: `json` emits `{changes}`; omitted prints one entry per line.
    #[arg(long)]
    pub(crate) format: Option<String>,
}

#[derive(Args)]
pub(crate) struct ChangesPathArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// A repo-relative path to act on; repeat for each one. Never a pathspec:
    /// the core refuses anything absent from the change set and git is invoked
    /// with `--literal-pathspecs`.
    #[arg(long)]
    pub(crate) path: Vec<String>,
}

#[derive(Args)]
pub(crate) struct ChangesCommitArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// The commit message, as ONE `--message=<msg>` token so a message beginning
    /// with `-` is never re-read as a flag.
    #[arg(long)]
    pub(crate) message: String,
}

/// `ralphy changes list|stage|unstage|commit`.
pub(crate) fn changes(cmd: ChangesCommand) -> anyhow::Result<()> {
    match cmd {
        ChangesCommand::List(args) => changes_list(args),
        ChangesCommand::Stage(args) => changes_stage(args),
        ChangesCommand::Unstage(args) => changes_unstage(args),
        ChangesCommand::Commit(args) => changes_commit(args),
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
