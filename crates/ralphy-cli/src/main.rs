//! Ralphy's command-line entry point and composition root: parse flags, resolve
//! the repo, build the queue, build the Claude adapter, and hand off to the core
//! queue lifecycle.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use ralphy_agent_claude::ClaudeAgent;
use ralphy_agent_codex::CodexAgent;
use ralphy_agent_opencode::OpenCodeAgent;
use ralphy_core::{
    git, github, run_queue, Agent, BranchMode, GhTracker, Outcome, QueueConfig, StopReason,
    WallClock, Workspace,
};
use tracing::{info, warn};

mod guard;
mod hook;
mod install;
mod runstate;
mod telegram;
mod ui;

#[derive(Parser)]
#[command(
    name = "ralphy",
    about = "Work a repo's GitHub issue queue with an agent CLI.",
    // Reports the git-published version captured by build.rs (e.g. `v0.1.0-rc2`),
    // not the Cargo manifest version. We bind lowercase `-v` (clap's default short is
    // the uppercase `-V`); the run flags use long-only `--verbose`, leaving `-v` free
    // at the top level. `disable_version_flag` drops clap's auto-generated `--version`
    // so the custom arg below is the sole owner of the flag.
    version = env!("RALPHY_VERSION"),
    disable_version_flag = true,
)]
struct Cli {
    /// Print the git-published version and exit.
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: (),

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Work the repo's issue queue onto a fresh run branch.
    Run(Box<RunArgs>),
    /// Consolidate the loose `.ralphy/knowledge/issue-<N>.md` notes into one
    /// curated `KNOWLEDGE.md` (an agent session merges, dedups, and verifies;
    /// consumed notes are archived under `knowledge/raw/`).
    Consolidate(ConsolidateArgs),
    /// Internal: agent-CLI hook handlers (invoked by the execution session, not
    /// by a human).
    #[command(subcommand)]
    Hook(HookCommand),
    /// Configure the optional Telegram run monitor (token, chat, status).
    #[command(subcommand)]
    Telegram(telegram::TelegramCommand),
    /// Symlink (or copy) this binary into a PATH directory so `ralphy` resolves
    /// from anywhere on the command line.
    Install(install::InstallArgs),
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

    /// Drop the animated presenter and print raw INFO `tracing` lines instead
    /// (also engaged by `RUST_LOG`/`RALPHY_LOG`). Useful for debugging or CI.
    #[arg(long)]
    verbose: bool,

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
    #[arg(long, default_value_t = 90)]
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

    /// Mute the Telegram run notifier for this run (no card, no pushes), even
    /// when Telegram is configured. See docs/adr/0007.
    #[arg(long)]
    no_telegram: bool,

    /// Override the auto-derived Telegram card title for this run.
    #[arg(long)]
    title: Option<String>,
}

#[derive(Args)]
struct ConsolidateArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    repo: PathBuf,

    /// Model for the consolidation session. Curation is judgment-heavy
    /// (dedup, conflict resolution, what to cut), so it defaults to opus.
    #[arg(long, default_value = "opus")]
    model: String,

    /// Reasoning effort for the consolidation session.
    #[arg(long, default_value = "medium")]
    effort: String,

    /// Wall-clock budget (minutes) before the session is reclaimed.
    #[arg(long, default_value_t = 30)]
    max_minutes: u64,
}

/// The CLI's own agent-selector enum so `clap` stays a CLI concern; the composition
/// root maps it to the boxed `&dyn Agent` it hands the core (docs/adr/0004 D1).
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum CliAgent {
    Claude,
    Codex,
    // The ADR-0005 contract and the documented invocation are `--agent opencode`
    // (one word). Clap would otherwise derive the kebab-cased `open-code` from the
    // variant name; pin the spelling and keep that derivation as an alias.
    #[value(name = "opencode", alias = "open-code")]
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
        Command::Consolidate(args) => consolidate_cmd(args),
        Command::Hook(HookCommand::Stop) => hook::run_stop_hook(),
        Command::Hook(HookCommand::Guard) => guard::run_guard_hook(),
        Command::Telegram(cmd) => telegram::run(cmd),
        Command::Install(args) => install::run(&args),
    }
}

/// The shared consolidation step behind both `ralphy consolidate` and the
/// automatic end-of-run trigger: run the curation session, verify it actually
/// rewrote `KNOWLEDGE.md`, and only then archive the consumed notes into
/// `knowledge/raw/`. Returns how many notes were archived. Errors — leaving the
/// notes loose for a retry — when the session left `KNOWLEDGE.md` missing or
/// unchanged. `notes` must be non-empty; callers gate on `loose_notes` first.
///
/// Callers are responsible for clearing `ANTHROPIC_API_KEY` (the subscription-quota
/// sentinel) before this runs — `run` already does so up front, `consolidate` does
/// it just before calling.
fn run_consolidation(
    ws: &Workspace,
    run_dir: &std::path::Path,
    model: Option<&str>,
    effort: Option<&str>,
    max_minutes: u64,
    notes: &[PathBuf],
) -> Result<usize> {
    use anyhow::bail;
    use ralphy_core::knowledge;

    std::fs::create_dir_all(run_dir).ok();

    // The curated file before the session, to verify the session produced one.
    let before = std::fs::read_to_string(ws.knowledge_file()).ok();

    ralphy_agent_claude::consolidate_knowledge(
        ws,
        run_dir,
        model,
        effort,
        std::time::Duration::from_secs(max_minutes * 60),
    )?;

    let after = std::fs::read_to_string(ws.knowledge_file()).ok();
    match after {
        Some(ref a) if before.as_deref() != Some(a.as_str()) => {}
        _ => bail!(
            "the session left KNOWLEDGE.md missing or unchanged — notes kept loose (see {})",
            run_dir.join("consolidate.log").display()
        ),
    }

    knowledge::archive_notes(ws, notes)
}

/// `ralphy consolidate`: run a one-shot agent session that curates the loose
/// knowledge notes into `KNOWLEDGE.md`, then archive the consumed notes under
/// `knowledge/raw/`. The session's only deliverable is the curated file — the
/// command verifies it actually changed before archiving anything, so a failed
/// or no-op session leaves the notes loose for a retry.
fn consolidate_cmd(args: ConsolidateArgs) -> Result<()> {
    use ralphy_core::knowledge;

    let repo_root = git::resolve_toplevel(&args.repo)?;
    let ws = Workspace::new(&repo_root);

    let notes = knowledge::loose_notes(&ws);
    if notes.is_empty() {
        println!("No loose knowledge notes under .ralphy/knowledge/ — nothing to consolidate.");
        return Ok(());
    }
    let names: Vec<String> = notes
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    println!(
        "Consolidating {} note(s) into KNOWLEDGE.md: {}",
        notes.len(),
        names.join(", ")
    );

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let run_dir = ws.run_dir(&stamp);

    // Same subscription-quota sentinel as `run` (see the comment there).
    std::env::set_var("ANTHROPIC_API_KEY", "");

    let archived = run_consolidation(
        &ws,
        &run_dir,
        non_empty(args.model).as_deref(),
        non_empty(args.effort).as_deref(),
        args.max_minutes,
        &notes,
    )?;
    println!(
        "Done: KNOWLEDGE.md updated, {archived} note(s) archived into .ralphy/knowledge/raw/."
    );
    Ok(())
}

fn run_cmd(args: RunArgs) -> Result<()> {
    let repo_root = git::resolve_toplevel(&args.repo)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let ws = Workspace::new(&repo_root);
    let run_dir = ws.run_dir(&stamp);
    std::fs::create_dir_all(&run_dir).ok();

    let log_file = std::fs::File::create(run_dir.join("ralphy.log")).ok();

    // Decide up front whether this run notifies (ADR-0007 D1/D7): only when
    // Telegram is configured (a token AND a captured chat) and the run is neither
    // `--no-telegram` nor a `--dry-run`. When it does, create the shared event ring
    // and install the notifier Layer alongside the file/presenter layers so it sees
    // the lifecycle from `queue built` onward. The worker is started later, once the
    // queue (and thus the title) is known.
    let tg_cfg = telegram::config::TelegramConfig::load().ok().flatten();
    let configured = tg_cfg.as_ref().is_some_and(|c| {
        c.chat_id.is_some() && telegram::config::effective_token(Some(&c.token)).is_some()
    });
    let notify = telegram::notifier::should_notify(configured, args.no_telegram, args.dry_run);
    let event_queue = notify.then(|| Arc::new(telegram::notifier::EventQueue::new()));
    let notifier_layer = event_queue
        .as_ref()
        .map(|q| telegram::notifier::NotifierLayer::new(q.clone()));

    let presenter = init_tracing(log_file, args.verbose, notifier_layer);

    // The repo name feeds the run title (below); the branding header is printed once
    // that title is known, so the console face is seeded by the same title as the
    // Telegram card — identical per run, varying across runs.
    let repo_name = repo_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo");

    info!(repo = %repo_root.display(), %stamp, dry_run = args.dry_run, "ralphy run");

    // Build the queue: the whole queue by default, or just `--only-issue` when set
    // (applied as a post-build filter, parity with the ps1 `$OnlyIssue`).
    std::fs::create_dir_all(ws.ralphy_dir()).ok();
    let effective_labels = github::resolve_queue_labels(&args.queue_label, &repo_root);
    let mut queue = github::list_queue(&effective_labels, &repo_root)?;
    if let Some(only) = args.only_issue {
        queue.retain(|i| i.number == only);
    }
    // Order by dependency (Blocked-by edges + split-bundle children), ascending
    // number as tie-break — the pending list shown to the user IS the sequence
    // run_queue will work, and a dependency-consistent order lets one run drain
    // a graph whose numbering disagrees with its edges.
    let queue = ralphy_core::blocked::sort_queue(queue);

    // Derive the run title once, before any on-screen line, so it can seed both the
    // console branding header and the Telegram card — the face then matches across
    // both surfaces and varies per run (a different queue → a different face).
    let only_issue_title = args.only_issue.and(queue.first()).map(|i| i.title.clone());
    let title = telegram::notifier::derive_title(
        repo_name,
        queue.len(),
        &effective_labels,
        only_issue_title.as_deref(),
        args.title.as_deref(),
    );

    // Branding header + info line, seeded by the run title (see above). All info-line
    // segments are best-effort — a detached HEAD or a local-only repo drops that part.
    presenter.print_header(&title);
    let start_branch = git::current_branch(&repo_root).ok();
    let repo_url = git::origin_url(&repo_root).map(|u| ui::normalize_remote_url(&u));
    presenter.print_info_line(repo_name, start_branch.as_deref(), repo_url.as_deref());

    if queue.is_empty() {
        let scope = match args.only_issue {
            Some(n) => format!("issue #{n}"),
            None => format!("labels [{}]", effective_labels.join(", ")),
        };
        // finalize before printing so the live region is cleared first (ADR-0006).
        presenter.finalize();
        presenter.print_notice(&format!("No open issues for {scope} to process. Done."));
        return Ok(());
    }
    let order: Vec<String> = queue.iter().map(|i| format!("#{}", i.number)).collect();
    // message consumed by the telegram notifier / presenter — keep stable
    info!(count = queue.len(), order = %order.join(" -> "), "queue built");

    // Start the Telegram notifier worker now that the queue (and thus the title)
    // is known. `try_start_notifier` runs `getMe`; on failure it warns once and
    // returns `None`, leaving the installed Layer inert and the run unaffected
    // (ADR-0007 D7). Events emitted before this point (just `queue built`) are
    // buffered in the ring and drained by the worker on start.
    let mut notifier: Option<telegram::notifier::NotifierHandle> = None;
    if let (Some(event_queue), Some(cfg)) = (event_queue.as_ref(), tg_cfg.as_ref()) {
        if let (Some(chat_id), Some(token)) = (
            cfg.chat_id,
            telegram::config::effective_token(Some(&cfg.token)),
        ) {
            let state = runstate::RunState::new(title.clone(), queue.len());
            let client =
                telegram::client::BotClient::new(telegram::client::UreqTransport::new(token));
            notifier =
                telegram::notifier::try_start_notifier(client, chat_id, state, event_queue.clone());
        }
    }

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

    let result = run_queue(&cfg, &queue, agent.as_ref(), &tracker, &clock);

    // Flush the queue bar to N/N and clear the live region before anything else
    // prints — whether that is the panel below or `anyhow`'s error on the `?`
    // propagation. Finalizing first keeps a `bail!` from being torn by a live bar
    // (ADR-0006: the presenter owns teardown).
    presenter.finalize();

    // Knowledge consolidation trigger: a non-dry run that finished (produced a
    // report) and left loose per-issue notes folds them into KNOWLEDGE.md, so the
    // curated cache the next run reads (prompt.execute.md reads KNOWLEDGE.md first)
    // stays current without a manual `consolidate` step. Everything lives under the
    // gitignored `.ralphy/`, so there is nothing to commit and the panel's "clean
    // run" report stays accurate. Run BEFORE the notifier shutdown and AFTER the
    // presenter finalize so it surfaces as a first-class lifecycle event in both
    // surfaces: the `info!`/`warn!` below decode to RunEvents the console presenter
    // renders (timestamp + 📚) and the live Telegram card folds (a 📚 line during,
    // a footer segment after). A failed session is a warning, never a run failure —
    // the run already succeeded and the notes stay loose for a later retry.
    // `ANTHROPIC_API_KEY` was already cleared up front; defaults mirror the
    // `consolidate` command (opus / medium / 30 min).
    if result.is_ok() && !args.dry_run {
        let notes = ralphy_core::knowledge::loose_notes(&ws);
        if !notes.is_empty() {
            info!(count = notes.len() as u64, "consolidating knowledge");
            let run_dir = ws.run_dir(&cfg.stamp);
            match run_consolidation(&ws, &run_dir, Some("opus"), Some("medium"), 30, &notes) {
                Ok(archived) => info!(count = archived as u64, "knowledge consolidated"),
                Err(e) => {
                    warn!(error = %e, "knowledge consolidation failed — notes kept loose for retry")
                }
            }
        }
    }

    // Tear down the notifier (ADR-0007 D4): signal the worker to render the
    // terminal state, send the final push, and flush, joined under a bounded
    // timeout so a wedged network never holds the process open. Done before the
    // `?` so a non-green result still triggers the final push.
    if let Some(notifier) = notifier.take() {
        notifier.shutdown();
    }

    let report = result?;

    // Bucket the worked issues into the three-way triad defined in the plan.
    let done = report
        .worked
        .iter()
        .filter(|r| r.outcome == Some(Outcome::Done))
        .count() as u64;
    let num_blocked = report
        .worked
        .iter()
        .filter(|r| r.outcome.is_some() && r.outcome != Some(Outcome::Done))
        .count() as u64;
    let skipped = report.worked.iter().filter(|r| r.outcome.is_none()).count() as u64;

    let panel_stop = report.stop.map(|s| match s {
        StopReason::Deadline => ui::PanelStop::Deadline,
        StopReason::NonGreen { number, outcome } => ui::PanelStop::NonGreen {
            number,
            outcome: format!("{outcome:?}"),
        },
        StopReason::StopBefore { number } => ui::PanelStop::StopBefore { number },
        StopReason::Limit { number, reset } => ui::PanelStop::Limit { number, reset },
    });

    let panel_mode = match branch_mode {
        BranchMode::New => ui::PanelBranchMode::New,
        BranchMode::Current => ui::PanelBranchMode::Current,
    };

    // Token-usage footer figures (ADR-0008 D11): the run total off this run's
    // accumulated usage, and the project's cumulative balance read from the
    // ledger. `cfg.repo_root` is still in scope (cfg owns it).
    let slug = git::project_slug(&cfg.repo_root);
    let run_tokens = report.run_usage.total();
    let project_tokens = ralphy_core::ledger::project_total(&slug).total();

    let data = ui::PanelData {
        branch: report.branch,
        orig_branch: report.orig_branch,
        done,
        blocked: num_blocked,
        skipped,
        commits: report.commits,
        stop: panel_stop,
        branch_mode: panel_mode,
        dry_run: args.dry_run,
        run_tokens,
        project_tokens,
        project_id: slug,
    };
    presenter.print_panel(&data);
    Ok(())
}

/// Force `stop_on_limit` for OpenCode runs only: OpenCode already self-waits short
/// limits and long ones carry no parseable reset, so auto-resume is never useful.
/// Claude and Codex pass the flag through unchanged — both emit a trustworthy reset
/// time (Codex an absolute RFC3339 instant, Claude a relative one), so both
/// auto-resume by default and honour `--stop-on-limit` as the opt-out.
fn effective_stop_on_limit(flag: bool, agent: CliAgent) -> bool {
    flag || matches!(agent, CliAgent::OpenCode)
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Install the tracing stack. The full structured log always goes to the run's
/// `ralphy.log` (no colour, local timestamps). On screen, the animated presenter
/// (ADR-0006) renders the run's lifecycle by default; `--verbose` (or a set
/// `RUST_LOG`/`RALPHY_LOG`) drops to raw INFO `fmt` lines and disables animation
/// so debugging is unobstructed.
///
/// Local timestamps everywhere fix the reported UTC-vs-local bug at the source:
/// the `fmt` layers use `ChronoLocal`, and the presenter composes its own local
/// timestamps via `chrono::Local`.
///
/// Always returns a `PresenterHandle` — `plain()` on the raw-stderr path (no colour,
/// no bars, no-op `finalize`), or the animated presenter's handle otherwise. The
/// caller routes the early-exit notice and the final panel through it uniformly.
fn init_tracing(
    log_file: Option<std::fs::File>,
    verbose: bool,
    notifier: Option<telegram::notifier::NotifierLayer>,
) -> ui::PresenterHandle {
    use tracing_subscriber::fmt::time::ChronoLocal;
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // `--verbose`, or any explicit env filter, means the operator wants the raw
    // log on screen — drop the presenter and disable animation (ADR-0006 D3).
    let raw_stderr = verbose
        || std::env::var_os("RUST_LOG").is_some()
        || std::env::var_os("RALPHY_LOG").is_some();

    let timer = ChronoLocal::new("%Y-%m-%d %H:%M:%S".to_string());

    // `ralphy.log` always carries the full uncoloured, local-time log.
    let file_layer = log_file.map(|file| {
        fmt::layer()
            .with_ansi(false)
            .with_timer(timer.clone())
            .with_writer(move || file.try_clone().expect("clone ralphy.log handle"))
    });

    // The notifier Layer (when installed) composes alongside the file/presenter
    // layers so it sees every consumed event; `Option<Layer>` is a no-op when None.
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(notifier);

    if raw_stderr {
        let stderr_layer = fmt::layer().with_timer(timer).with_writer(std::io::stderr);
        registry.with(stderr_layer).init();
        ui::PresenterHandle::plain()
    } else {
        let presenter = ui::Presenter::new();
        let handle = presenter.handle();
        registry.with(presenter).init();
        handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_stop_on_limit_codex_passes_flag_through() {
        // Codex emits an absolute RFC3339 reset, so it auto-resumes by default like
        // Claude — the flag is no longer forced on.
        assert!(!effective_stop_on_limit(false, CliAgent::Codex));
        assert!(effective_stop_on_limit(true, CliAgent::Codex));
    }

    #[test]
    fn effective_stop_on_limit_claude_passes_flag_through() {
        assert!(!effective_stop_on_limit(false, CliAgent::Claude));
        assert!(effective_stop_on_limit(true, CliAgent::Claude));
    }

    #[test]
    fn cli_agent_accepts_opencode_spelling() {
        // The documented invocation is `--agent opencode` (one word, ADR-0005 D1).
        // Guard against clap silently reverting to the kebab-cased `open-code`.
        use clap::ValueEnum;
        assert_eq!(
            CliAgent::from_str("opencode", false).ok(),
            Some(CliAgent::OpenCode)
        );
        // The derived kebab spelling stays accepted as an alias.
        assert_eq!(
            CliAgent::from_str("open-code", false).ok(),
            Some(CliAgent::OpenCode)
        );
    }

    #[test]
    fn effective_stop_on_limit_opencode_forces_true() {
        assert!(effective_stop_on_limit(false, CliAgent::OpenCode));
        assert!(effective_stop_on_limit(true, CliAgent::OpenCode));
    }
}
