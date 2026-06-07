//! Ralphy's command-line entry point and composition root: parse flags, resolve
//! the repo, fetch the issue, build the Claude adapter, and hand off to the core
//! run lifecycle.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use ralphy_agent_claude::ClaudeAgent;
use ralphy_core::{git, github, run, RunConfig, RunOutcome, Workspace};
use tracing::info;

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
    /// Plan (and, later, execute) one issue onto a fresh run branch.
    Run(RunArgs),
}

#[derive(Args)]
struct RunArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    repo: PathBuf,

    /// Work only this issue number.
    #[arg(long)]
    only_issue: u64,

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run_cmd(args),
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

    // Fetch the issue and persist it where the planner reads it (.ralphy/ is
    // gitignored, so it survives the branch checkout the core does next).
    std::fs::create_dir_all(ws.ralphy_dir()).ok();
    let issue_json = github::fetch_issue_json(args.only_issue)?;
    std::fs::write(ws.issue_json_path(), &issue_json).context("writing .ralphy/issue.json")?;
    let issue = github::parse_issue(&issue_json)?;
    info!(number = issue.number, title = %issue.title, "issue fetched");

    let agent = ClaudeAgent::new(
        non_empty(args.plan_model),
        non_empty(args.plan_effort),
        run_dir,
    );
    let cfg = RunConfig {
        repo_root,
        base_branch: args.base_branch,
        dry_run: args.dry_run,
        stamp,
    };

    let report = run(&cfg, &issue, &agent)?;
    match report.outcome {
        RunOutcome::DryRun { open_steps } => {
            info!(branch = %report.branch, open_steps, restored = %report.orig_branch, "dry-run complete");
            println!(
                "Dry-run complete: {} open step(s) planned in {} (repo restored to '{}').",
                open_steps,
                ws.plan_path().display(),
                report.orig_branch
            );
        }
        RunOutcome::Executed(outcome) => {
            println!("Run finished on '{}': {:?}", report.branch, outcome);
        }
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
