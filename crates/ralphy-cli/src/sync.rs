//! `ralphy sync status|fetch|pull` — the branch's relation to its upstream and
//! the two acts that change it. Every primitive delegates to an already-public
//! [`ralphy_core::sync`] function; this module is only the guard + clap surface.
//!
//! `status` is read-only and never consults `.ralphy/run.lock` (see
//! `changes.rs`). `fetch` and `pull` inspect it and refuse under
//! [`crate::runlock::LockState::HeldAlive`] BEFORE any git call (ADR-0036 §6).
//!
//! Neither write takes `--format`: a refusal reaches the workbench only through
//! the non-zero-exit message path (the daemon's Mutate branch collapses a
//! successful exit to `{"status":"ok"}` and discards stdout), so the outcome's
//! own prose is carried on stderr by exiting non-zero.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use ralphy_core::sync::{FetchOutcome, Head, PullOutcome};

use crate::runlock;
use crate::runlock::guard_run_lock;

#[derive(Subcommand)]
pub(crate) enum SyncCommand {
    /// Report branch, upstream and ahead/behind (read-only, no network call;
    /// never consults the run.lock).
    Status(SyncStatusArgs),
    /// Fetch the branch's remote (refuses under a held run.lock).
    Fetch(SyncArgs),
    /// Fast-forward from the upstream, never merging (refuses under a held run.lock).
    Pull(SyncArgs),
}

#[derive(Args)]
pub(crate) struct SyncStatusArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// Output format: `json` emits `{sync}`; omitted prints one human line.
    #[arg(long)]
    pub(crate) format: Option<String>,
}

#[derive(Args)]
pub(crate) struct SyncArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,
}

/// `ralphy sync status|fetch|pull`.
pub(crate) fn sync(cmd: SyncCommand) -> anyhow::Result<()> {
    match cmd {
        SyncCommand::Status(args) => sync_status(args),
        SyncCommand::Fetch(args) => sync_fetch(args),
        SyncCommand::Pull(args) => sync_pull(args),
    }
}

/// `ralphy sync status [--format json]`. Read-only: no run-lock guard.
fn sync_status(args: SyncStatusArgs) -> anyhow::Result<()> {
    let repo_root = ralphy_core::git::resolve_toplevel(&args.repo)?;
    let st = ralphy_core::sync::status(&repo_root)?;

    if args.format.as_deref() == Some("json") {
        let out = serde_json::json!({ "sync": st });
        println!("{out}");
    } else {
        println!("{}", human_line(&st));
    }
    Ok(())
}

/// The one-line human form. An absent upstream reads as its own words, never as
/// zeroed counts.
fn human_line(st: &ralphy_core::sync::SyncStatus) -> String {
    let stamp = match &st.last_fetch {
        Some(when) => format!("last fetch {when}"),
        None => "never fetched".to_string(),
    };
    match (&st.head, &st.tracking) {
        (Head::Detached { sha }, _) => format!("detached at {sha} — {stamp}"),
        (Head::Branch { name }, None) => format!("{name} (no upstream) — {stamp}"),
        (Head::Branch { name }, Some(t)) => format!(
            "{name} [{}] {} ahead, {} behind — {stamp}",
            t.upstream, t.ahead, t.behind
        ),
    }
}

/// Resolve the toplevel and refuse under a live run lock — in that order, and
/// before any `ralphy_core::sync` call, so a guarded verb reaches no git.
fn guarded_root(repo: &Path, verb: &str) -> anyhow::Result<PathBuf> {
    let repo_root = ralphy_core::git::resolve_toplevel(repo)?;
    let ws = ralphy_core::Workspace::new(&repo_root);
    guard_run_lock(&ws, verb, runlock::pid_is_alive)?;
    Ok(repo_root)
}

/// `ralphy sync fetch`. A refusal is a non-zero exit carrying the core's prose.
fn sync_fetch(args: SyncArgs) -> anyhow::Result<()> {
    let repo_root = guarded_root(&args.repo, "sync fetch")?;
    match ralphy_core::sync::fetch(&repo_root)? {
        FetchOutcome::Fetched { remote } => println!("Fetched {remote}."),
        refused => anyhow::bail!(
            "{}",
            refused.reason().unwrap_or_else(|| format!("{refused:?}"))
        ),
    }
    Ok(())
}

/// `ralphy sync pull`. Fast-forward only; anything else refuses by value.
fn sync_pull(args: SyncArgs) -> anyhow::Result<()> {
    let repo_root = guarded_root(&args.repo, "sync pull")?;
    match ralphy_core::sync::pull(&repo_root)? {
        PullOutcome::UpToDate => println!("Already up to date."),
        PullOutcome::FastForwarded { commits } => println!("Fast-forwarded {commits} commits."),
        refused => anyhow::bail!(
            "{}",
            refused.reason().unwrap_or_else(|| format!("{refused:?}"))
        ),
    }
    Ok(())
}
