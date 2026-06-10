//! Ralphy's command-line entry point and composition root: parse flags, resolve
//! the repo, build the queue, build the Claude adapter, and hand off to the core
//! queue lifecycle.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use ralphy_agent_claude::ClaudeAgent;
use ralphy_agent_codex::CodexAgent;
use ralphy_agent_opencode::OpenCodeAgent;
use ralphy_core::{
    git, github, run_queue, Agent, BranchMode, GhTracker, QueueConfig, StopReason, WallClock,
    Workspace,
};
use tracing::info;

mod guard;
mod hook;

#[derive(Parser)]
#[command(
    name = "ralphy",
    about = "Work a repo's GitHub issue queue with an agent CLI."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Work the repo's issue queue onto a fresh run branch.
    Run(Box<RunArgs>),
    /// Internal: agent-CLI hook handlers (invoked by the execution session, not
    /// by a human).
    #[command(subcommand)]
    Hook(HookCommand),
}

#[derive(Subcommand)]
enum HookCommand {
    /// Stop hook: record the session's exit sentinel to `$RALPHY_FLAG_FILE`.
    Stop,
    /// PreToolUse guard: block destructive commands/writes.
    Guard,
}

#[derive(Args)]
struct RunArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    repo: PathBuf,

    /// Which agent CLI drives the run: `claude` (the default, a live PTY session)
    /// or `codex` (headless `codex exec`). Selected per run; the core never learns
    /// which vendor it holds.
    #[arg(long = "agent", value_enum, default_value_t = CliAgent::Claude)]
    agent: CliAgent,

    /// Work only this issue number (filters the queue to it). Omit to work the
    /// whole queue.
    #[arg(long)]
    only_issue: Option<u64>,

    /// Queue label(s): an open issue carrying ANY of these is worked. Repeatable.
    /// When omitted, defaults to ["ready-for-agent", "AFK"] plus any label
    /// mapped from "ready-for-agent" in docs/agents/triage-labels.md.
    #[arg(long = "queue-label")]
    queue_label: Vec<String>,

    /// Global wall-clock budget (hours): don't start a new issue past it. Omit
    /// for no deadline.
    #[arg(long)]
    deadline_hours: Option<f64>,

    /// Plan only; make no source changes and drop the empty run branch.
    #[arg(long)]
    dry_run: bool,

    /// Commit-ish the run branch is cut from. Only used with `--branch-mode new`.
    #[arg(long, default_value = "origin/main")]
    base_branch: String,

    /// Where commits land: `new` cuts a fresh `afk/run-*` branch off
    /// `--base-branch`; `current` commits straight onto the branch the repo is
    /// already on (no new branch, `--base-branch` ignored).
    #[arg(long = "branch-mode", value_enum, default_value_t = CliBranchMode::New)]
    branch_mode: CliBranchMode,

    /// Planning model.
    #[arg(long, default_value = "opus")]
    plan_model: String,

    /// Planning effort.
    #[arg(long, default_value = "medium")]
    plan_effort: String,

    /// Force the execution model for the issue (overrides the plan's judgment).
    #[arg(long)]
    exec_model: Option<String>,

    /// OpenCode `--variant` (effort) passed through to `opencode run`. Omitted
    /// when unset so the adapter never sends a value the provider rejects
    /// (docs/adr/0005 D3). Only used by `--agent opencode`.
    #[arg(long)]
    exec_variant: Option<String>,

    /// Execution effort.
    #[arg(long, default_value = "medium")]
    exec_effort: String,

    /// Execution model used when the plan emits no complexity judgment.
    #[arg(long, default_value = "sonnet")]
    default_exec_model: String,

    /// Per-issue wall-clock budget (minutes) before the session is reclaimed.
    #[arg(long, default_value_t = 45)]
    max_minutes_per_issue: u64,

    /// Enable Remote Control so you can follow/intervene from the mobile app
    /// (the default).
    #[arg(long, overrides_with = "no_remote_control")]
    remote_control: bool,

    /// Disable Remote Control for the execution session.
    #[arg(long = "no-remote-control", overrides_with = "remote_control")]
    no_remote_control: bool,

    /// Use a `claude -p` loop instead of an interactive PTY session (for
    /// environments with no TTY, e.g. CI).
    #[arg(long)]
    headless_exec: bool,

    /// Maximum number of `claude -p` calls per issue before declaring stuck
    /// (headless mode only).
    #[arg(long, default_value_t = 6)]
    max_exec_calls: u32,

    /// On a usage limit, stop and report the reset instead of the default
    /// (wait for the reset and auto-resume the same issue). See docs/adr/0003.
    #[arg(long)]
    stop_on_limit: bool,
}

/// The CLI's own agent-selector enum so `clap` stays a CLI concern; the composition
/// root maps it to the boxed `&dyn Agent` it hands the core (docs/adr/0004 D1).
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum CliAgent {
    Claude,
    Codex,
    OpenCode,
}

/// The CLI's own branch-mode enum so `clap` stays a CLI concern; it converts into
/// the core's `BranchMode` (see docs/adr/0002).
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum CliBranchMode {
    New,
    Current,
}

impl From<CliBranchMode> for BranchMode {
    fn from(m: CliBranchMode) -> Self {
        match m {
            CliBranchMode::New => BranchMode::New,
            CliBranchMode::Current => BranchMode::Current,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run_cmd(*args),
        Command::Hook(HookCommand::Stop) => hook::run_stop_hook(),
        Command::Hook(HookCommand::Guard) => guard::run_guard_hook(),
    }
}

fn run_cmd(args: RunArgs) -> Result<()> {
    let repo_root = git::resolve_toplevel(&args.repo)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let ws = Workspace::new(&repo_root);
    let run_dir = ws.run_dir(&stamp);
    std::fs::create_dir_all(&run_dir).ok();

    let log_file = std::fs::File::create(run_dir.join("ralphy.log")).ok();
    init_tracing(log_file);

    info!(repo = %repo_root.display(), %stamp, dry_run = args.dry_run, "ralphy run");

    // Build the queue: the whole queue by default, or just `--only-issue` when set
    // (applied as a post-build filter, parity with the ps1 `$OnlyIssue`).
    std::fs::create_dir_all(ws.ralphy_dir()).ok();
    let effective_labels = github::resolve_queue_labels(&args.queue_label, &repo_root);
    let mut queue = github::list_queue(&effective_labels, &repo_root)?;
    if let Some(only) = args.only_issue {
        queue.retain(|i| i.number == only);
    }
    if queue.is_empty() {
        let scope = match args.only_issue {
            Some(n) => format!("issue #{n}"),
            None => format!("labels [{}]", effective_labels.join(", ")),
        };
        println!("No open issues for {scope} to process. Done.");
        return Ok(());
    }
    let order: Vec<String> = queue.iter().map(|i| format!("#{}", i.number)).collect();
    info!(count = queue.len(), order = %order.join(" -> "), "queue built");

    // Clear any inherited ANTHROPIC_API_KEY so the agent draws on the subscription
    // quota (matching the ps1 oracle behaviour).
    //
    // Why `""` and not `remove_var`: setting to the empty string deliberately
    // mirrors the PowerShell oracle, which assigns `""` rather than unsetting.
    // Claude's handling of an absent key vs. an empty key is not verified, so we
    // keep the same sentinel value to stay on the tested path.
    //
    // Single-threaded safety: `set_var` is safe to call here because this point
    // in `main` is reached before any threads are spawned; no concurrent reader
    // can observe a torn environment state.
    //
    // Edition-2024 migration note: in Rust edition 2024, `std::env::set_var` (and
    // `remove_var`) become `unsafe` functions. When the crate migrates to edition
    // 2024, this call must be wrapped in an `unsafe` block with a comment
    // reiterating the single-threaded safety argument above.
    std::env::set_var("ANTHROPIC_API_KEY", "");

    // The run's global wall-clock deadline (if any), shared by the agent — which
    // clamps each issue's budget to it — and the queue's between-issue clock.
    let run_deadline = args
        .deadline_hours
        .map(|h| std::time::Instant::now() + std::time::Duration::from_secs_f64(h * 3600.0));

    // Select the adapter per run and box it as `&dyn Agent`; the core takes a
    // single `&dyn Agent` and never learns which vendor it holds (docs/adr/0004).
    let agent: Box<dyn Agent> = match args.agent {
        CliAgent::Claude => Box::new(
            ClaudeAgent::new(
                non_empty(args.plan_model),
                non_empty(args.plan_effort),
                run_dir,
            )
            .with_exec_config(
                non_empty(args.exec_model.unwrap_or_default()),
                non_empty(args.exec_effort),
                args.default_exec_model,
                args.max_minutes_per_issue,
                !args.no_remote_control,
                args.headless_exec,
                args.max_exec_calls,
            )
            .with_run_deadline(run_deadline),
        ),
        CliAgent::Codex => Box::new(
            CodexAgent::new(non_empty(args.exec_model.unwrap_or_default()), run_dir)
                .with_run_deadline(run_deadline),
        ),
        CliAgent::OpenCode => Box::new(
            OpenCodeAgent::new(non_empty(args.exec_model.unwrap_or_default()), run_dir)
                .with_variant(non_empty(args.exec_variant.unwrap_or_default()))
                .with_run_deadline(run_deadline),
        ),
    };
    let branch_mode: BranchMode = args.branch_mode.into();
    let cfg = QueueConfig {
        repo_root,
        base_branch: args.base_branch,
        dry_run: args.dry_run,
        stamp,
        branch_mode,
        only_issue: args.only_issue,
        stop_on_limit: effective_stop_on_limit(args.stop_on_limit, args.agent),
    };

    // The same deadline gates starting the next issue (between-issue clock).
    let clock = WallClock {
        deadline: run_deadline,
    };
    let tracker = GhTracker::new(cfg.repo_root.clone());

    let report = run_queue(&cfg, &queue, agent.as_ref(), &tracker, &clock)?;

    // Per-issue summary, then how the run ended and where the branch stands.
    println!(
        "Run on '{}' (was on '{}'):",
        report.branch, report.orig_branch
    );
    for r in &report.worked {
        let status = match (&r.outcome, r.closed) {
            (Some(o), true) => format!("{o:?}, closed"),
            (Some(o), false) => format!("{o:?}"),
            (None, _) if !r.blocked_by.is_empty() => {
                let refs: Vec<String> = r.blocked_by.iter().map(|n| format!("#{n}")).collect();
                format!("skipped (blocked by {})", refs.join(", "))
            }
            (None, _) if args.dry_run => "planned (dry-run)".to_string(),
            (None, _) => "skipped (infeasible)".to_string(),
        };
        println!("  #{}: {status}", r.number);
    }
    let stopped = report.stop.is_some();
    match report.stop {
        Some(StopReason::Deadline) => {
            println!("Stopped: run deadline reached (before the next issue, or a usage-limit reset landed past it).")
        }
        Some(StopReason::NonGreen { number, outcome }) => {
            println!("Stopped: #{number} finished non-green ({outcome:?}). Branch handed back.");
        }
        Some(StopReason::StopBefore { number }) => {
            println!(
                "Stopped: stop-before label on #{number}. Remove the label and re-run to continue."
            );
        }
        Some(StopReason::Limit { number, reset }) => {
            // With auto-resume the default, a surfaced Limit means either
            // `--stop-on-limit` was set, the reset was unparseable, or the
            // progress-aware cap abandoned the issue.
            print!("Stopped: usage limit on #{number}.");
            match reset {
                Some(t) => {
                    print!(" Reset ~{t}; re-run to continue (or it stalled with no progress).")
                }
                None => print!(" No parseable reset time; re-run after the limit clears."),
            }
            println!();
        }
        None => println!("Queue complete."),
    }

    // Commit count over the compare ref and the oneline log, then the closing
    // state per mode/outcome — mirrors the ps1 `finally` block.
    println!(
        "Branch '{}' carries {} commit(s).",
        report.branch, report.commits
    );
    for line in &report.oneline {
        println!("    {line}");
    }
    match branch_mode {
        BranchMode::Current => {
            if args.dry_run {
                println!("DryRun on '{}': no commits made.", report.branch);
            } else if stopped {
                println!("Left repo on '{}' for inspection.", report.branch);
            } else {
                println!(
                    "Clean run: {} commit(s) added to '{}' in place.",
                    report.commits, report.branch
                );
            }
        }
        BranchMode::New => {
            if args.dry_run {
                println!(
                    "DryRun: returned repo to '{}'; empty run branch removed.",
                    report.orig_branch
                );
            } else if stopped {
                println!(
                    "Left repo checked out on '{}' for inspection.",
                    report.branch
                );
            } else {
                println!(
                    "Clean run: returned repo to '{}'. Run branch '{}' kept.",
                    report.orig_branch, report.branch
                );
            }
            if !(args.dry_run && report.commits == 0) {
                println!(
                    "Review, then merge '{}' into your target:  git merge {}",
                    report.branch, report.branch
                );
            }
        }
    }
    Ok(())
}

/// Force `stop_on_limit` for Codex runs: Codex's rolling reset window is not
/// parseable, so auto-resume is never useful there. Claude passes the flag
/// through unchanged.
fn effective_stop_on_limit(flag: bool, agent: CliAgent) -> bool {
    flag || matches!(agent, CliAgent::Codex)
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Log to stderr, and additionally to the run's `ralphy.log` when available.
fn init_tracing(log_file: Option<std::fs::File>) {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr_layer = fmt::layer().with_writer(std::io::stderr);
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer);

    match log_file {
        Some(file) => {
            let file_layer = fmt::layer()
                .with_ansi(false)
                .with_writer(move || file.try_clone().expect("clone ralphy.log handle"));
            registry.with(file_layer).init();
        }
        None => registry.init(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_stop_on_limit_codex_forces_true() {
        assert!(effective_stop_on_limit(false, CliAgent::Codex));
        assert!(effective_stop_on_limit(true, CliAgent::Codex));
    }

    #[test]
    fn effective_stop_on_limit_claude_passes_flag_through() {
        assert!(!effective_stop_on_limit(false, CliAgent::Claude));
        assert!(effective_stop_on_limit(true, CliAgent::Claude));
    }
}
