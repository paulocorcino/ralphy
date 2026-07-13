//! Run-lock-aware git branch ops and label mutation (ADR-0036 §6): `ralphy
//! branch switch`, `ralphy branch create`, and `ralphy label set`. Each verb
//! inspects `.ralphy/run.lock` (`crate::runlock`) and refuses under
//! [`runlock::LockState::HeldAlive`] before making any `git`/`gh` call — a
//! mutation reached before the guard defeats its purpose (ADR-0036 §6). Every
//! primitive here delegates to an already-public `ralphy_core` function; this
//! module is only the guard + clap surface.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::runlock;

/// Refuses `verb` when a run currently holds the repo's lock; every other
/// state (free, stale, corrupt) is safe to proceed — a dead PID or an
/// unreadable lock is not a live run.
fn guard_run_lock(
    ws: &ralphy_core::Workspace,
    verb: &str,
    is_alive: impl Fn(u32) -> bool,
) -> anyhow::Result<()> {
    if let runlock::LockState::HeldAlive(info) = runlock::inspect(&ws.run_lock_path(), is_alive) {
        anyhow::bail!(
            "refusing to {verb}: a run holds this repo's lock (pid {}, since {}) — wait for it to finish or stop it",
            info.pid,
            info.started_at
        );
    }
    Ok(())
}

/// `label set` requires at least one `--add`/`--remove`; an invocation with
/// neither is a no-op that would otherwise silently succeed.
fn require_some_label(add: &[String], remove: &[String]) -> anyhow::Result<()> {
    if add.is_empty() && remove.is_empty() {
        anyhow::bail!("label set: pass at least one --add <label> or --remove <label>");
    }
    Ok(())
}

#[derive(Subcommand)]
pub(crate) enum BranchCommand {
    /// Check out an existing branch (refuses under a held run.lock).
    Switch(BranchArgs),
    /// Create a branch from the current HEAD (refuses under a held run.lock).
    Create(BranchArgs),
}

#[derive(Args)]
pub(crate) struct BranchArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// The branch to switch to / create.
    #[arg(value_name = "NAME")]
    pub(crate) name: String,
}

#[derive(Subcommand)]
pub(crate) enum LabelCommand {
    /// Add/remove label(s) on an issue via the forge (refuses under a held run.lock).
    Set(LabelSetArgs),
}

#[derive(Args)]
pub(crate) struct LabelSetArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// The issue number to mutate.
    #[arg(value_name = "ISSUE")]
    pub(crate) issue: u64,

    /// Label(s) to add. Repeatable.
    #[arg(long)]
    pub(crate) add: Vec<String>,

    /// Label(s) to remove. Repeatable.
    #[arg(long)]
    pub(crate) remove: Vec<String>,
}

/// `ralphy branch switch|create <name>`.
pub(crate) fn branch(cmd: BranchCommand) -> anyhow::Result<()> {
    let (args, verb, is_create) = match cmd {
        BranchCommand::Switch(a) => (a, "branch switch", false),
        BranchCommand::Create(a) => (a, "branch create", true),
    };
    let repo_root = ralphy_core::git::resolve_toplevel(&args.repo)?;
    let ws = ralphy_core::Workspace::new(&repo_root);
    guard_run_lock(&ws, verb, runlock::pid_is_alive)?;

    if is_create {
        ralphy_core::git::checkout_new_branch(&repo_root, &args.name, "HEAD")?;
        println!("Created and switched to branch '{}'.", args.name);
    } else {
        ralphy_core::git::checkout(&repo_root, &args.name)?;
        println!("Switched to branch '{}'.", args.name);
    }
    Ok(())
}

/// `ralphy label set <issue> [--add <L>]... [--remove <L>]...`.
pub(crate) fn label(cmd: LabelCommand) -> anyhow::Result<()> {
    let LabelCommand::Set(args) = cmd;
    require_some_label(&args.add, &args.remove)?;

    let repo_root = ralphy_core::git::resolve_toplevel(&args.repo)?;
    let ws = ralphy_core::Workspace::new(&repo_root);
    guard_run_lock(&ws, "label set", runlock::pid_is_alive)?;

    for l in &args.remove {
        ralphy_core::github::remove_label(args.issue, l, &repo_root)?;
    }
    for l in &args.add {
        ralphy_core::github::add_label(args.issue, l, &repo_root)?;
    }
    println!(
        "Issue #{}: removed {:?}, added {:?}.",
        args.issue, args.remove, args.add
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Hand-rolled unique temp dir (same idiom as `runlock.rs`'s `tmp_lock`).
    fn tmp_ws(name: &str) -> ralphy_core::Workspace {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ralphy-mutate-{}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            name
        ));
        fs::create_dir_all(dir.join(".ralphy")).unwrap();
        ralphy_core::Workspace::new(&dir)
    }

    #[test]
    fn guard_refuses_under_held_alive() {
        let ws = tmp_ws("held");
        let stored = runlock::LockInfo {
            pid: 4_000_000,
            started_at: "2026-07-13T10:00:00-03:00".into(),
        };
        fs::write(ws.run_lock_path(), serde_json::to_string(&stored).unwrap()).unwrap();

        let err = guard_run_lock(&ws, "branch switch", |pid| pid == 4_000_000)
            .unwrap_err()
            .to_string();
        assert!(err.contains("refusing to branch switch"), "got: {err}");
        assert!(err.contains("4000000"), "got: {err}");
    }

    #[test]
    fn guard_allows_when_free() {
        let ws = tmp_ws("free");
        assert!(guard_run_lock(&ws, "branch switch", |_| true).is_ok());
    }

    #[test]
    fn guard_allows_when_stale() {
        let ws = tmp_ws("stale");
        let stored = runlock::LockInfo {
            pid: 4_000_001,
            started_at: "2026-07-13T10:00:00-03:00".into(),
        };
        fs::write(ws.run_lock_path(), serde_json::to_string(&stored).unwrap()).unwrap();

        assert!(guard_run_lock(&ws, "branch switch", |_| false).is_ok());
    }

    #[test]
    fn require_some_label_rejects_empty() {
        assert!(require_some_label(&[], &[]).is_err());
    }

    #[test]
    fn require_some_label_accepts_add() {
        assert!(require_some_label(&["x".to_string()], &[]).is_ok());
    }
}
