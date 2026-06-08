//! Ralphy's command-line entry point and composition root: parse flags, resolve
//! the repo, build the queue, build the Claude adapter, and hand off to the core
//! queue lifecycle.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use ralphy_agent_claude::ClaudeAgent;
use ralphy_core::{
    git, github, run_queue, GhTracker, QueueConfig, StopReason, WallClock, Workspace,
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

    /// Work only this issue number (filters the queue to it). Omit to work the
    /// whole queue.
    #[arg(long)]
    only_issue: Option<u64>,

    /// Queue label(s): an open issue carrying ANY of these is worked. Repeatable.
    #[arg(long = "queue-label", default_values_t = [String::from("ready-for-agent"), String::from("AFK")])]
    queue_label: Vec<String>,

    /// Global wall-clock budget (hours): don't start a new issue past it. Omit
    /// for no deadline.
    #[arg(long)]
    deadline_hours: Option<f64>,

    /// Plan only; make no source changes and drop the empty run branch.
    #[arg(long)]
    dry_run: bool,

    /// Commit-ish the run branch is cut from.
    #[arg(long, default_value = "origin/main")]
    base_branch: String,

    /// Planning model.
    #[arg(long, default_value = "opus")]
    plan_model: String,

    /// Planning effort.
    #[arg(long, default_value = "medium")]
    plan_effort: String,

    /// Force the execution model for the issue (overrides the plan's judgment).
    #[arg(long)]
    exec_model: Option<String>,

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
    let mut queue = github::list_queue(&args.queue_label)?;
    if let Some(only) = args.only_issue {
        queue.retain(|i| i.number == only);
    }
    if queue.is_empty() {
        let scope = match args.only_issue {
            Some(n) => format!("issue #{n}"),
            None => format!("labels [{}]", args.queue_label.join(", ")),
        };
        println!("No open issues for {scope} to process. Done.");
        return Ok(());
    }
    let order: Vec<String> = queue.iter().map(|i| format!("#{}", i.number)).collect();
    info!(count = queue.len(), order = %order.join(" -> "), "queue built");

    // Guarantee subscription billing: clear any inherited API key for this run
    // (as the ps1 oracle does), so the agent draws on the subscription quota.
    std::env::set_var("ANTHROPIC_API_KEY", "");

    let agent = ClaudeAgent::new(
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
    );
    let cfg = QueueConfig {
        repo_root,
        base_branch: args.base_branch,
        dry_run: args.dry_run,
        stamp,
    };

    // Build the global deadline (if any) from --deadline-hours.
    let clock = WallClock {
        deadline: args
            .deadline_hours
            .map(|h| std::time::Instant::now() + std::time::Duration::from_secs_f64(h * 3600.0)),
    };
    let tracker = GhTracker;

    let report = run_queue(&cfg, &queue, &agent, &tracker, &clock)?;

    // Per-issue summary, then how the run ended and where the branch stands.
    println!(
        "Run on '{}' (was on '{}'):",
        report.branch, report.orig_branch
    );
    for r in &report.worked {
        let status = match (&r.outcome, r.closed) {
            (Some(o), true) => format!("{o:?}, closed"),
            (Some(o), false) => format!("{o:?}"),
            (None, _) if args.dry_run => "planned (dry-run)".to_string(),
            (None, _) => "skipped (infeasible)".to_string(),
        };
        println!("  #{}: {status}", r.number);
    }
    match report.stop {
        Some(StopReason::Deadline) => println!("Stopped: deadline reached before the next issue."),
        Some(StopReason::NonGreen { number, outcome }) => {
            println!("Stopped: #{number} finished non-green ({outcome:?}). Branch handed back.");
        }
        None => println!("Queue complete."),
    }
    Ok(())
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
