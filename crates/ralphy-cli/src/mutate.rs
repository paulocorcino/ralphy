//! Run-lock-aware git branch ops and label mutation (ADR-0036 §6): `ralphy
//! branch switch`, `ralphy branch create`, `ralphy worktree add` (ADR-0063 §2),
//! `ralphy worktree remove` (ADR-0063 §1's gates), `ralphy label set` — plus the reads beside them, `ralphy branch list` and
//! `ralphy worktree list` (ADR-0063 §1), which never consult the lock. Each mutating verb
//! inspects `.ralphy/run.lock` (`crate::runlock`) and refuses under
//! [`runlock::LockState::HeldAlive`] before making any `git`/`gh` call — a
//! mutation reached before the guard defeats its purpose (ADR-0036 §6). Every
//! primitive here delegates to an already-public `ralphy_core` function; this
//! module is only the guard + clap surface.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};

use crate::runlock;
use crate::runlock::guard_run_lock;

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
    /// Switch to a branch that exists. Refused while a run is working in the
    /// repo.
    Switch(BranchArgs),
    /// Create a branch from the current commit. Refused while a run is working
    /// in the repo.
    Create(BranchArgs),
    /// List the repo's local branches. Changes nothing.
    List(BranchListArgs),
}

#[derive(Args)]
pub(crate) struct BranchListArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// Output: text (the default, with `* ` before the current branch) or
    /// `json`.
    #[arg(long)]
    pub(crate) format: Option<String>,
}

#[derive(Args)]
pub(crate) struct BranchArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// The branch name.
    #[arg(value_name = "NAME")]
    pub(crate) name: String,
}

#[derive(Subcommand)]
pub(crate) enum WorktreeCommand {
    /// List the worktrees under `.ralphy/worktrees/`.
    List(WorktreeListArgs),
    /// Create a worktree with a new branch under `.ralphy/worktrees/`. Refused
    /// while a run is working in the repo.
    Add(WorktreeAddArgs),
    /// Remove a worktree and its branch. Refused while a run is working in the
    /// repo, when the worktree has uncommitted changes, or when its branch is
    /// not merged.
    Remove(WorktreeRemoveArgs),
}

#[derive(Args)]
pub(crate) struct WorktreeRemoveArgs {
    /// Any folder inside the repo or inside one of its worktrees.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// The worktree's name (also the name of its branch).
    #[arg(value_name = "NAME")]
    pub(crate) name: String,
}

#[derive(Args)]
pub(crate) struct WorktreeAddArgs {
    /// Any folder inside the repo or inside one of its worktrees.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// The worktree's name. It is also the name of its folder under
    /// `.ralphy/worktrees/` and of its new branch.
    #[arg(value_name = "NAME")]
    pub(crate) name: String,

    /// The branch or commit that the new branch starts from. Default: the
    /// current branch of the repo's main folder.
    #[arg(long, value_name = "REF")]
    pub(crate) base: Option<String>,
}

#[derive(Args)]
pub(crate) struct WorktreeListArgs {
    /// Any folder inside the repo or inside one of its worktrees.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// Output: text (the default) or `json`.
    #[arg(long)]
    pub(crate) format: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum LabelCommand {
    /// Add or remove labels on an issue. Refused while a run is working in the
    /// repo.
    Set(LabelSetArgs),
}

#[derive(Args)]
pub(crate) struct LabelSetArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// The issue number.
    #[arg(value_name = "ISSUE")]
    pub(crate) issue: u64,

    /// A label to add (repeat for more).
    #[arg(long)]
    pub(crate) add: Vec<String>,

    /// A label to remove (repeat for more).
    #[arg(long)]
    pub(crate) remove: Vec<String>,
}

/// `ralphy branch switch|create <name>`.
pub(crate) fn branch(cmd: BranchCommand) -> anyhow::Result<()> {
    let (args, verb, is_create) = match cmd {
        BranchCommand::Switch(a) => (a, "branch switch", false),
        BranchCommand::Create(a) => (a, "branch create", true),
        // A read never blocks on the run lock, so `List` skips `guard_run_lock`.
        BranchCommand::List(a) => return branch_list(a),
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

/// `ralphy branch list [--format json]`. Read-only: no run-lock guard.
fn branch_list(args: BranchListArgs) -> anyhow::Result<()> {
    let repo_root = ralphy_core::git::resolve_toplevel(&args.repo)?;
    let current = ralphy_core::git::current_branch(&repo_root)?;
    let branches = ralphy_core::git::local_branches(&repo_root)?;

    if args.format.as_deref() == Some("json") {
        let out = serde_json::json!({ "current": current, "branches": branches });
        println!("{out}");
    } else {
        for b in &branches {
            if *b == current {
                println!("* {b}");
            } else {
                println!("  {b}");
            }
        }
    }
    Ok(())
}

/// `ralphy worktree list|add|remove`.
pub(crate) fn worktree(cmd: WorktreeCommand) -> anyhow::Result<()> {
    match cmd {
        // A read never blocks on the run lock, so `List` skips `guard_run_lock`.
        WorktreeCommand::List(args) => worktree_list(args),
        WorktreeCommand::Add(args) => worktree_add(args),
        WorktreeCommand::Remove(args) => worktree_remove(args),
    }
}

/// `ralphy worktree remove <name>`. The run lock is the primary tree's, so the
/// guard runs against the primary; then ADR-0063 §1's gates in `remove` —
/// locked, dirty, no `--force`, `branch -d` never `-D`. A refusal is returned
/// bare (no `.context`), so stderr is the one line the daemon relays verbatim.
fn worktree_remove(args: WorktreeRemoveArgs) -> anyhow::Result<()> {
    let start = ralphy_core::git::resolve_toplevel(&args.repo)?;
    let primary = ralphy_core::checkouts::primary(&start)?;
    let ws = ralphy_core::Workspace::new(&primary);
    guard_run_lock(&ws, "worktree remove", runlock::pid_is_alive)?;
    ralphy_core::checkouts::remove(&primary, &args.name)?;
    println!(
        "Removed worktree '{}' and branch '{}'.",
        args.name, args.name
    );
    Ok(())
}

/// `ralphy worktree add <name> [--base <ref>]`. The run lock is the primary
/// tree's, so the guard runs against the primary — before `add`, the only
/// writer.
fn worktree_add(args: WorktreeAddArgs) -> anyhow::Result<()> {
    let start = ralphy_core::git::resolve_toplevel(&args.repo)?;
    let primary = ralphy_core::checkouts::primary(&start)?;
    let ws = ralphy_core::Workspace::new(&primary);
    guard_run_lock(&ws, "worktree add", runlock::pid_is_alive)?;
    // Carry-over is warn-only end to end: a settings file that will not
    // parse costs the carry-over, never the worktree.
    let (settings, settings_warning) = match ralphy_core::settings::Settings::load(&ws) {
        Ok(s) => (s, None),
        Err(e) => (
            ralphy_core::settings::Settings::default(),
            Some(format!("settings.json not read, carry-over skipped: {e:#}")),
        ),
    };
    let c = ralphy_core::checkouts::add(&primary, &args.name, args.base.as_deref())?;
    println!(
        "Created worktree '{}' at {} from {}.",
        c.name, c.path, c.base
    );
    // Carry-over runs AFTER the add reported: the worktree exists whatever
    // these say, and the daemon relays this stdout as the picker's notice.
    if let Some(w) = settings_warning {
        println!("warning: {w}");
    }
    for warning in
        ralphy_core::checkouts::carry_over(&primary, Path::new(&c.path), &settings.worktree)
    {
        println!("warning: {warning}");
    }
    Ok(())
}

/// `ralphy worktree list [--format json]`. Read-only: no run-lock guard.
fn worktree_list(args: WorktreeListArgs) -> anyhow::Result<()> {
    let repo_root = ralphy_core::git::resolve_toplevel(&args.repo)?;
    let listing = ralphy_core::checkouts::list(&repo_root)?;

    if args.format.as_deref() == Some("json") {
        println!("{}", serde_json::to_string(&listing)?);
    } else {
        println!("* {}", listing.primary);
        for w in &listing.worktrees {
            let dirty = if w.dirty {
                "  (uncommitted changes)"
            } else {
                ""
            };
            println!("  {}  {}{dirty}", w.name, w.branch);
        }
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

    #[test]
    fn label_set_rejects_empty_labels_before_touching_repo() {
        // A nonexistent --repo would fail `resolve_toplevel` with "not a git
        // repository"; the arg-validation error must win, proving
        // `require_some_label` runs BEFORE `resolve_toplevel`.
        let err = label(LabelCommand::Set(LabelSetArgs {
            repo: PathBuf::from("/definitely-not-a-repo-xyz"),
            issue: 1,
            add: vec![],
            remove: vec![],
        }))
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("pass at least one --add"),
            "expected the label-arg error, got: {err}"
        );
    }
}
