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

mod config;
mod events;
mod guard;
mod hook;
mod init;
mod install;
mod issues;
mod models;
mod pricing;
mod runlock;
mod runstate;
mod split_agent;
mod telegram;
mod triage;
mod ui;
mod usage;

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
    /// Read the project's token ledger: print the balance and group-by cuts
    /// (`--by phase|model|actor|version`, `--since`, `--project`), or export it
    /// (`--format csv|json`). USD is a read-time projection, never stored.
    Usage(usage::UsageArgs),
    /// List models available to an agent (`--agent opencode`, the default).
    /// Passes through to the agent's own model-listing command.
    Models(models::ModelsArgs),
    /// Configure per-repo operator settings (e.g. `opencode.model`).
    Config(config::ConfigArgs),
    /// Configure the optional Telegram run monitor (token, chat, status).
    #[command(subcommand)]
    Telegram(telegram::TelegramCommand),
    /// Symlink (or copy) this binary into a PATH directory so `ralphy` resolves
    /// from anywhere on the command line.
    Install(install::InstallArgs),
    /// Validate the environment prerequisites for a repo: Python, `gh` auth, a
    /// GitHub remote, and at least one logged-in agent CLI (ADR-0012 stage 1).
    Init(init::InitArgs),
    /// Agent-triage the `triage-agent` issues (ADR-0017): promote, consolidate,
    /// or bounce each, previewed before publishing (`--yes` for schedulers).
    Triage(triage::TriageArgs),
    /// Read-only backlog query: list open issues as the runner judges them, or
    /// `issues show <n>` for one issue's detail (ADR-0020). `--format json` /
    /// `--fields` for machine output.
    Issues(issues::IssuesArgs),
}

#[derive(Subcommand)]
enum HookCommand {
    /// Stop hook: record the session's exit sentinel to `$RALPHY_FLAG_FILE`.
    Stop,
    /// PreToolUse guard: block destructive commands/writes.
    Guard,
    /// PostToolUse (Bash): record measured verify-command durations for the
    /// verification-cost gate.
    Post,
}

#[derive(Args)]
struct RunArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    repo: PathBuf,

    /// Which agent CLI executes the run: `claude` (the default, a live PTY
    /// session), `codex` (headless `codex exec`), or `opencode`. Selects the
    /// executor; pair with `--plan-agent` to plan with a different adapter.
    /// Selected per run; the core never learns which vendor it holds.
    #[arg(long = "agent", value_enum, default_value_t = CliAgent::Claude)]
    agent: CliAgent,

    /// Adapter for the planning phase; defaults to `--agent` when omitted, so a
    /// single-agent run is unchanged. The canonical split is
    /// `--agent opencode --plan-agent claude` (Claude plans, OpenCode executes).
    /// Any planner/executor combination is accepted (ADR-0009).
    #[arg(long = "plan-agent", value_enum)]
    plan_agent: Option<CliAgent>,

    /// Work only this issue number (filters the queue to it). Omit to work the
    /// whole queue.
    #[arg(long)]
    only_issue: Option<u64>,

    /// Work exactly these issues, in the order given, ignoring queue labels:
    /// `--issues 5,3,9`. Each number is fetched directly (no label filter, no
    /// dependency re-ordering), so the run drains the list as a sequence. Like
    /// `--only-issue`, a `stop-before` label on a listed issue is ignored;
    /// unlike it, human-return labels (ADR-0016) are still respected. Mutually
    /// exclusive with `--only-issue`.
    #[arg(long, value_delimiter = ',', conflicts_with = "only_issue")]
    issues: Vec<u64>,

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

    /// Commit-ish the run branch is cut from. Only used with `--branch-mode new`
    /// (default: origin/main, or `base_branch` in settings.json).
    #[arg(long)]
    base_branch: Option<String>,

    /// Where commits land: `new` cuts a fresh `afk/run-*` branch off
    /// `--base-branch`; `current` commits straight onto the branch the repo is
    /// already on (no new branch, `--base-branch` ignored). (default: new, or
    /// `branch_mode` in settings.json).
    #[arg(long = "branch-mode", value_enum)]
    branch_mode: Option<CliBranchMode>,

    /// Planning model (default: opus, or `claude.plan_model` in settings.json).
    #[arg(long)]
    plan_model: Option<String>,

    /// Planning effort (default: medium, or `claude.plan_effort` in
    /// settings.json).
    #[arg(long)]
    plan_effort: Option<String>,

    /// Force the execution model for the issue (overrides the plan's judgment).
    #[arg(long)]
    exec_model: Option<String>,

    /// OpenCode `--variant` (effort) passed through to `opencode run`. Omitted
    /// when unset so the adapter never sends a value the provider rejects
    /// (docs/adr/0005 D3). Only used by `--agent opencode`.
    #[arg(long)]
    exec_variant: Option<String>,

    /// Execution effort (default: medium, or `claude.exec_effort` in
    /// settings.json).
    #[arg(long)]
    exec_effort: Option<String>,

    /// Execution model used when the plan emits no complexity judgment
    /// (default: sonnet, or `claude.default_exec_model` in settings.json).
    #[arg(long)]
    default_exec_model: Option<String>,

    /// Per-issue wall-clock budget (minutes) before the session is reclaimed
    /// (default: unbounded, or `claude.max_minutes_per_issue` in settings.json).
    /// `0` (the default) disables the cap — the issue is then bounded only by
    /// `--deadline-hours`. Set a positive value to opt into a per-issue cap.
    #[arg(long)]
    max_minutes_per_issue: Option<u64>,

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

    /// Skip this invocation (exit 0) when another run is already active in the
    /// repo — the anti-overlap flag scheduled invocations pass so a timer never
    /// piles a run onto a live one. Without it a live run only warns.
    #[arg(long)]
    if_idle: bool,

    /// Build the label queue only from issues this login is among the assignees
    /// of (`gh --assignee` semantics; `@me` = the authenticated user). Overrides a
    /// persisted `queue.assignee`. `--only-issue`/`--issues` ignore this filter.
    #[arg(long)]
    assignee: Option<String>,

    /// Disable a persisted `queue.assignee` filter for this one invocation, so the
    /// queue is built unfiltered. Mutually exclusive with `--assignee`.
    #[arg(long = "no-assignee", conflicts_with = "assignee")]
    no_assignee: bool,
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

impl CliAgent {
    fn cli_name(self) -> &'static str {
        match self {
            CliAgent::Claude => "claude",
            CliAgent::Codex => "codex",
            CliAgent::OpenCode => "opencode",
        }
    }
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

/// The five Claude-only run knobs resolved once (flag > settings.json >
/// hardcoded default, ADR-0010) so the executor and an optional split planner
/// share one value. Strings are guaranteed non-empty by the resolvers.
struct ResolvedClaude {
    plan_model: String,
    plan_effort: String,
    exec_effort: String,
    default_exec_model: String,
    max_minutes_per_issue: u64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run_cmd(*args),
        Command::Consolidate(args) => consolidate_cmd(args),
        Command::Models(args) => models::run(args),
        Command::Config(args) => config::run(args),
        Command::Usage(args) => usage::usage_cmd(args),
        Command::Hook(HookCommand::Stop) => hook::run_stop_hook(),
        Command::Hook(HookCommand::Guard) => guard::run_guard_hook(),
        Command::Hook(HookCommand::Post) => hook::run_post_hook(),
        Command::Telegram(cmd) => telegram::run(cmd),
        Command::Install(args) => install::run(&args),
        Command::Init(args) => init::run(&args),
        Command::Triage(args) => triage::run(&args),
        Command::Issues(args) => issues::issues_cmd(args),
    }
}

/// The shared consolidation step behind both `ralphy consolidate` and the
/// automatic end-of-run trigger: run the curation session, verify it actually
/// rewrote `KNOWLEDGE.md` AND that the result passes the structural gate
/// (`knowledge::validate_knowledge`), then archive ONLY the notes the session
/// declared folded (its `<!-- folded: ... -->` marker) into `knowledge/raw/` —
/// unfolded notes stay loose, named in a warning, for the next pass. Returns
/// how many notes were archived. Errors — leaving every note loose for a retry
/// and restoring the pre-session `KNOWLEDGE.md` — when the session left the
/// file missing, unchanged, or structurally malformed (the rejected output is
/// kept as `KNOWLEDGE.rejected.md` in the run dir for inspection). `notes`
/// must be non-empty; callers gate on `loose_notes` first.
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
    use anyhow::{bail, Context};
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
    let after = match after {
        Some(a) if before.as_deref() != Some(a.as_str()) => a,
        _ => bail!(
            "the session left KNOWLEDGE.md missing or unchanged — notes kept loose (see {})",
            run_dir.join("consolidate.log").display()
        ),
    };

    // Structural gate: a truncated/mangled file must not count as success. On
    // rejection restore the pre-session curated file (a mangled one would
    // poison every reader until the next consolidation) and keep the rejected
    // output beside the log for inspection.
    let folded = match knowledge::validate_knowledge(&after) {
        Ok(folded) => folded,
        Err(e) => {
            let _ = std::fs::write(run_dir.join("KNOWLEDGE.rejected.md"), &after);
            let restore = match &before {
                Some(b) => std::fs::write(ws.knowledge_file(), b),
                None => std::fs::remove_file(ws.knowledge_file()),
            };
            restore.context("restoring the pre-session KNOWLEDGE.md")?;
            bail!(
                "the session produced a malformed KNOWLEDGE.md ({e:#}) — change rejected, \
                 notes kept loose (rejected file kept at {})",
                run_dir.join("KNOWLEDGE.rejected.md").display()
            );
        }
    };

    let (to_archive, leftover) = knowledge::partition_folded(notes, &folded);
    if !leftover.is_empty() {
        let names: Vec<String> = leftover
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        warn!(
            notes = %names.join(", "),
            "notes not folded by the session — kept loose for the next pass"
        );
    }
    knowledge::archive_notes(ws, &to_archive)
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
    // Anchors the `run.finished` `duration_s` (ADR-0019) — the run's wall-clock.
    let run_start = std::time::Instant::now();
    let repo_root = git::resolve_toplevel(&args.repo)?;
    let plan_agent = resolve_plan_agent(args.plan_agent, args.agent);
    preflight_agents(args.agent, plan_agent)?;
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

    // The CloudEvents sink (ADR-0019): active only when this repo has an
    // `events.url` in the global store (`~/.ralphy/events.toml`) — an absent entry
    // means non-users pay nothing. Build the ring + Layer here so the sink sees the
    // lifecycle from `queue built` onward; the worker starts once the run context
    // is known (below). The token honours `RALPHY_EVENTS_TOKEN` over the stored one.
    let events_slug = git::project_slug(&repo_root);
    let events_entry = events::config::EventsStore::load()
        .ok()
        .unwrap_or_default()
        .entry(&events_slug)
        .cloned();
    let events_url = events_entry.as_ref().and_then(|e| e.url.clone());
    let events_token =
        events::config::effective_token(events_entry.as_ref().and_then(|e| e.token.as_deref()));
    // Strip RALPHY_EVENTS_TOKEN from the process env now that the effective token is
    // captured in `events_token` (an owned String the sink transport keeps using):
    // every child spawned later inherits this environment and none must see the
    // sink's bearer token (ADR-0019). Done HERE — before init_tracing installs the
    // layers and before any worker thread is spawned — so the `remove_var` runs
    // single-threaded, with no concurrent `getenv` to race (edition 2021).
    strip_events_token_from_env();
    let event_sink_queue = events_url.as_ref().map(|_| events::sink::new_queue());
    let events_layer = event_sink_queue
        .as_ref()
        .map(|q| events::sink::EventsLayer::new(q.clone()));

    let presenter = init_tracing(log_file, args.verbose, notifier_layer, events_layer);

    // The repo name feeds the run title (below); the branding header is printed once
    // that title is known, so the console face is seeded by the same title as the
    // Telegram card — identical per run, varying across runs.
    let repo_name = repo_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo");

    info!(repo = %repo_root.display(), %stamp, dry_run = args.dry_run, "ralphy run");

    std::fs::create_dir_all(ws.ralphy_dir()).ok();

    // Presence lock (issue #72): the concurrency policy lives in the invocation.
    // `--if-idle` defers to a live run (clean exit 0, so a scheduler's history
    // shows no false failures); without it a live lock only warns — intentional
    // concurrency stays the human's call. Stale locks (dead PID after a crash
    // or reboot) are taken over so a crash never silences a schedule.
    match runlock::inspect(&ws.run_lock_path(), runlock::pid_is_alive) {
        runlock::LockState::HeldAlive(info) if args.if_idle => {
            let since = chrono::DateTime::parse_from_rfc3339(&info.started_at)
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or(info.started_at);
            let msg = format!("skipped: run in progress since {since}, pid {}", info.pid);
            info!("{msg}");
            // finalize before printing so the live region is cleared first (ADR-0006).
            presenter.finalize();
            presenter.print_notice(&msg);
            return Ok(());
        }
        runlock::LockState::HeldAlive(info) => {
            warn!(
                pid = info.pid,
                since = %info.started_at,
                "a run is already active in this repo — proceeding anyway"
            );
        }
        runlock::LockState::Stale(info) => {
            warn!(
                pid = info.pid,
                "ignoring stale run.lock (process not running)"
            );
        }
        runlock::LockState::Corrupt => warn!("ignoring unreadable run.lock"),
        runlock::LockState::Free => {}
    }
    // Held for the rest of run_cmd; Drop removes the file on every exit path.
    let _run_lock = runlock::acquire(&ws.run_lock_path())?;

    // The issues the operator named explicitly: `--issues 5,3,9` verbatim, or the
    // single `--only-issue N` folded into a one-element list. Empty = the ordinary
    // label-built queue. Handed to the core so `stop-before` is ignored on exactly
    // these issues (parity with the ps1 `$OnlyIssue` guard, generalized to a set).
    let forced_issues: Vec<u64> = if !args.issues.is_empty() {
        args.issues.clone()
    } else {
        args.only_issue.into_iter().collect()
    };

    // Build the queue. Two paths:
    //   `--issues`: an explicit, ordered selection — fetch each number directly
    //     (label-agnostic, no dependency re-ordering) and run the list AS GIVEN, so
    //     the run drains it as a sequence. This bypasses the label question entirely.
    //   default: the label-built queue, optionally narrowed by `--only-issue`, then
    //     ordered by dependency.
    // Load the persisted settings once here (ADR-0010) BEFORE the queue build so the
    // `queue.assignee` default is available to the label-queue path (and, further
    // down, the events ctx). A load failure warns and falls back to defaults so a
    // malformed settings file never aborts a run. Precedence for each knob: per-run
    // flag > settings.json > hardcoded default.
    let settings = match ralphy_core::Settings::load(&ws) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "could not load .ralphy/settings.json — persisted defaults ignored");
            ralphy_core::Settings::default()
        }
    };
    // The effective assignee filter for the label-built queue (ADR-0021): flag >
    // `--no-assignee` > persisted `queue.assignee` > none. `--only-issue`/`--issues`
    // bypass it below so an explicit selection always fetches unfiltered.
    let assignee = config::resolve_assignee(
        args.assignee.as_deref(),
        args.no_assignee,
        settings.queue.assignee.as_deref(),
    );

    let effective_labels = github::resolve_queue_labels(&args.queue_label, &repo_root);
    let queue = if !args.issues.is_empty() {
        let mut selected = Vec::with_capacity(args.issues.len());
        for number in &args.issues {
            selected.push(github::fetch_issue(*number, &repo_root)?);
        }
        selected
    } else {
        // `--only-issue` fetches its single target unfiltered (criterion 5), so drop
        // the assignee filter on that path; a bare label queue applies it.
        let queue_assignee = if args.only_issue.is_some() {
            None
        } else {
            assignee.as_deref()
        };
        let mut queue = github::list_queue(&effective_labels, queue_assignee, &repo_root)?;
        if let Some(only) = args.only_issue {
            queue.retain(|i| i.number == only);
        }
        // Order by dependency (Blocked-by edges + split-bundle children), ascending
        // number as tie-break — the pending list shown to the user IS the sequence
        // run_queue will work, and a dependency-consistent order lets one run drain
        // a graph whose numbering disagrees with its edges.
        //
        // Fetch the full open-issue set so a blocker that sits OUTSIDE the queue but is
        // itself open (a partially-labelled chain) still orders the queue: edges are
        // walked transitively through those out-of-queue nodes. Best-effort — on a `gh`
        // failure fall back to in-queue-only ordering rather than abort the run. Skip
        // the extra call when ordering can't matter (0 or 1 issue).
        if queue.len() > 1 {
            match github::list_open_issues(&repo_root) {
                Ok(open) => ralphy_core::blocked::sort_queue_in_graph(queue, &open),
                Err(e) => {
                    warn!(error = %e, "could not list open issues for dependency ordering; using in-queue edges only");
                    ralphy_core::blocked::sort_queue(queue)
                }
            }
        } else {
            ralphy_core::blocked::sort_queue(queue)
        }
    };

    // Derive the run title once, before any on-screen line, so it can seed both the
    // console branding header and the Telegram card — the face then matches across
    // both surfaces and varies per run (a different queue → a different face).
    // A single named issue (`--only-issue N`, or `--issues N` with one entry)
    // titles the card with that issue's own title; a multi-issue list falls back
    // to the "N issues" form.
    let single_title = if forced_issues.len() == 1 {
        queue.first().map(|i| i.title.clone())
    } else {
        None
    };
    // An explicit `--issues` selection isn't label-scoped, so don't tag the card
    // with the (unused) default labels.
    let title_labels: &[String] = if args.issues.is_empty() {
        &effective_labels
    } else {
        &[]
    };
    let title = telegram::notifier::derive_title(
        repo_name,
        queue.len(),
        title_labels,
        single_title.as_deref(),
        args.title.as_deref(),
    );

    // Branding header + info line, seeded by the run title (see above). All info-line
    // segments are best-effort — a detached HEAD or a local-only repo drops that part.
    presenter.print_header(&title);
    let start_branch = git::current_branch(&repo_root).ok();
    let repo_url = git::origin_url(&repo_root).map(|u| ui::normalize_remote_url(&u));
    presenter.print_info_line(repo_name, start_branch.as_deref(), repo_url.as_deref());

    if queue.is_empty() {
        let scope = empty_queue_scope(
            &args.issues,
            args.only_issue,
            &effective_labels,
            assignee.as_deref(),
        );
        // finalize before printing so the live region is cleared first (ADR-0006).
        presenter.finalize();
        presenter.print_notice(&format!("No open issues for {scope} to process. Done."));
        return Ok(());
    }
    let order: Vec<String> = queue.iter().map(|i| format!("#{}", i.number)).collect();
    // Where the run will halt: the first issue carrying `stop-before` in the sorted
    // order (0 = none). An explicit selection (`--only-issue`/`--issues`) overrides
    // the label, so the cut never applies there. Emitted so the pending bar can mark
    // the boundary up front (the run won't touch that issue or anything after it).
    // Mirrors the runner's gate in runner.rs.
    // The CLI shares the runner's exact predicate (`first_stop_before`) so the
    // boundary marker on the pending bar and the runner's own gate can never
    // disagree; `0` is the "no stop-before" sentinel the event decoder expects.
    let stop_before = ralphy_core::first_stop_before(&queue, &forced_issues).unwrap_or(0);

    // The human-return label set (ADR-0016): resolved once here and reused both for
    // the enriched queue snapshot below and the `gh`-free core (`QueueConfig`).
    let human_return_labels = github::resolve_human_return_labels(&repo_root);

    // ADR-0020: enrich `queue built` with the per-issue snapshot the runner would
    // judge, so the CloudEvents sink carries `data.issues[]` on `queue.built` and a
    // remote consumer sees the backlog for free. Best-effort — a resolver error (a
    // `gh` blip on a blocked-by probe) warns and emits the legacy shape rather than
    // aborting a run over its own telemetry.
    let issues_json = {
        let tracker = GhTracker::new(&repo_root);
        match ralphy_core::resolve_queue_view(
            &queue,
            &forced_issues,
            &human_return_labels,
            &tracker,
        ) {
            Ok(view) => serde_json::to_string(&view.issues).unwrap_or_default(),
            Err(e) => {
                warn!(error = %e, "resolving the queue snapshot failed — emitting the legacy queue.built shape");
                String::new()
            }
        }
    };
    // ADR-0021 §5: mark the queue's assignee scope on `queue.built`. The filter is
    // the *applied* one — `None` on an explicit `--issues`/`--only-issue` selection
    // (which bypasses the assignee filter above), matching how those paths fetch
    // unfiltered. Resolution is best-effort telemetry: a post-retry `gh api user`
    // failure warns and drops the scope mark to `null` rather than aborting the run
    // (the same rule that governs `issues_json`), and the concrete login is emitted,
    // never the literal `@me`.
    let applied_assignee = if !args.issues.is_empty() || args.only_issue.is_some() {
        None
    } else {
        assignee.clone()
    };
    let assignee_filter: Option<String> = match applied_assignee.as_deref() {
        Some(a) => match github::resolve_login(a, &repo_root) {
            Ok(l) => Some(l),
            Err(e) => {
                warn!(error = %e, "resolving @me for the assignee_filter mark failed — emitting queue.built without the scope mark");
                None
            }
        },
        None => None,
    };
    // message consumed by the telegram notifier / presenter — keep stable
    info!(
        count = queue.len(),
        order = %order.join(" -> "),
        stop_before,
        issues_json = %issues_json,
        assignee_filter = %assignee_filter.as_deref().unwrap_or(""),
        "queue built"
    );

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

    // Settings were loaded above (before the queue build) so `queue.assignee` could
    // filter the label queue; the operating run branch resolved from them still feeds
    // the events ctx below (ADR-0019 amendment #96), constant from the first event.
    // Each adapter's settings section is opaque JSON to the core; deserialize
    // the typed slices here with the same warn-and-default tolerance as the
    // file load, so a malformed section never aborts a run (ADR-0002 amendment).
    let claude_settings: ralphy_agent_claude::ClaudeSettings = settings
        .agent_settings(ralphy_agent_claude::ClaudeSettings::SECTION)
        .unwrap_or_else(|e| {
            warn!(error = %e, "malformed claude settings section — its persisted defaults ignored");
            Default::default()
        });
    let opencode_settings: ralphy_agent_opencode::OpenCodeSettings = settings
        .agent_settings(ralphy_agent_opencode::OpenCodeSettings::SECTION)
        .unwrap_or_else(|e| {
            warn!(error = %e, "malformed opencode settings section — its persisted defaults ignored");
            Default::default()
        });
    let persisted_opencode_model = opencode_settings.model.clone();
    let base_branch = config::resolve_str(
        args.base_branch.clone(),
        settings.base_branch.clone(),
        "origin/main",
    );
    let branch_mode: BranchMode = args
        .branch_mode
        .map(BranchMode::from)
        .or_else(|| {
            settings
                .branch_mode
                .as_deref()
                .and_then(|m| config::parse_branch_mode(m).ok())
        })
        .unwrap_or(BranchMode::New);
    // The branch commits land on: a fresh `afk/run-<stamp>` in `new` mode (matching
    // the format literal in `runner.rs`), the current branch in `current` mode.
    let operating_branch = operating_branch(branch_mode, &stamp, start_branch.as_deref());

    // Start the CloudEvents sink worker now that the run context is known. The
    // process `runid` and the emitter identity are minted once here; the worker
    // drains the ring (already buffering `queue built`) and POSTs each event as a
    // CloudEvents envelope. A spawn failure leaves the installed Layer inert.
    let mut events_handle: Option<events::sink::EventsHandle> = None;
    if let (Some(queue), Some(url)) = (event_sink_queue.as_ref(), events_url.as_ref()) {
        let ctx = events::envelope::EventCtx {
            source: events::emitter::source(&events_slug),
            runid: events::emitter::new_runid(),
            emitter: serde_json::to_value(events::emitter::detect(&repo_root)).unwrap_or_default(),
            git: serde_json::json!({
                "repository": events_slug,
                "branch": operating_branch,
            }),
        };
        let transport = events::client::UreqEventTransport::new(url.clone(), events_token.clone());
        events_handle = events::sink::try_start_sink(transport, ctx, queue.clone(), ws.plan_path());
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

    // Select the adapter(s) per run and box as `&dyn Agent`; the core takes a
    // single `&dyn Agent` and never learns which vendor it holds (docs/adr/0004).
    // `--plan-agent` defaults to `--agent`: when they match, the executor box is
    // handed to the core directly so the single-agent path carries no wrapper
    // (byte-for-byte unchanged); otherwise a `SplitAgent` routes plan→planner,
    // execute→executor (docs/adr/0009).
    //
    // The persisted settings snapshot + `base_branch`/`branch_mode` were resolved
    // above (before the events ctx, so the operating branch is constant from the
    // first event); every run knob below resolves against that same snapshot.
    // ADR-0019 run-boundary event: emitted here, after branch-mode/base-branch
    // resolution, so it carries the CLI-only run parameters the core never sees.
    // The stream order is `queue.built` (above) then `run.started`; consumers order
    // by `id`/`time` and tolerate it (docs/events.md).
    let branch_mode_str = match branch_mode {
        BranchMode::New => "new",
        BranchMode::Current => "current",
    };
    info!(
        repo = %events_slug,
        queue_labels = %effective_labels.join(","),
        agent = args.agent.cli_name(),
        plan_agent = plan_agent.cli_name(),
        branch_mode = branch_mode_str,
        base = %base_branch,
        deadline_hours = args.deadline_hours.unwrap_or(0.0),
        "run started"
    );
    let resolved_claude = ResolvedClaude {
        plan_model: config::resolve_str(
            args.plan_model.clone(),
            claude_settings.plan_model.clone(),
            "opus",
        ),
        plan_effort: config::resolve_str(
            args.plan_effort.clone(),
            claude_settings.plan_effort.clone(),
            "medium",
        ),
        exec_effort: config::resolve_str(
            args.exec_effort.clone(),
            claude_settings.exec_effort.clone(),
            "medium",
        ),
        default_exec_model: config::resolve_str(
            args.default_exec_model.clone(),
            claude_settings.default_exec_model.clone(),
            "sonnet",
        ),
        max_minutes_per_issue: config::resolve_u64(
            args.max_minutes_per_issue,
            claude_settings.max_minutes_per_issue,
            ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE,
        ),
    };
    let executor = build_agent(
        args.agent,
        &args,
        run_dir.clone(),
        run_deadline,
        persisted_opencode_model.clone(),
        &resolved_claude,
    );
    let agent: Box<dyn Agent> = if plan_agent == args.agent {
        executor
    } else {
        Box::new(split_agent::SplitAgent {
            planner: build_agent(
                plan_agent,
                &args,
                run_dir,
                run_deadline,
                persisted_opencode_model,
                &resolved_claude,
            ),
            executor,
        })
    };
    // `human_return_labels` (ADR-0016) was resolved once above (for the enriched
    // queue snapshot) and is handed here to the `gh`-free core, which skips any
    // queued issue carrying one of these and continues the queue.
    let cfg = QueueConfig {
        repo_root,
        base_branch,
        dry_run: args.dry_run,
        stamp,
        branch_mode,
        forced_issues,
        human_return_labels,
        // Per-phase limit stance, each derived from the agent serving that phase;
        // an explicit `--stop-on-limit` forces both (docs/adr/0009).
        stop_on_limit_plan: effective_stop_on_limit(args.stop_on_limit, plan_agent),
        stop_on_limit_exec: effective_stop_on_limit(args.stop_on_limit, args.agent),
        // The runner-enforced verify gate (ADR-0011): the per-repo fallback
        // command (tokenized into one argv) used only when a plan emits no
        // `## Verify` section, and the gate's time budget (the per-issue budget).
        verify_fallback: settings
            .verify
            .command
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(|c| vec![ralphy_core::verify::tokenize(c)]),
        // The verify gate borrows the per-issue budget, but a disabled budget
        // (`0` = no per-issue cap) must not collapse the gate to a 0s timeout —
        // fall back to the default window so verify still has room to run.
        verify_timeout: std::time::Duration::from_secs(
            match resolved_claude.max_minutes_per_issue {
                0 => ralphy_core::VERIFY_GATE_FALLBACK_MINUTES,
                n => n,
            }
            .saturating_mul(60),
        ),
        // ADR-0015: when set, a `Done` that resolves to no verify gate at all is
        // parked for a human (`ready-for-human`) instead of closed on the
        // self-report. Absent/false preserves the ADR-0011 warn-and-close.
        require_verify_gate: settings.verify.require_verify_gate.unwrap_or(false),
        // The completion token the core quotes in its repair briefs. Named once
        // at the adapter layer; the core receives it as data and never learns
        // the literal (ADR-0002 amendment, #79).
        done_signal: ralphy_adapter_support::DONE_SENTINEL.to_owned(),
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
    // ADR-0019 run-boundary event: emitted only on a CLEAN termination (`run_queue`
    // returned `Ok`) — a crash/kill is detected by heartbeat silence, never a
    // `run.finished`. Emitted BEFORE the sink shutdown so the worker drains and
    // POSTs it as the run's last event.
    if let Ok(report) = result.as_ref() {
        let issues_done = report
            .worked
            .iter()
            .filter(|r| r.outcome == Some(Outcome::Done))
            .count() as u64;
        // The generic skip bucket (a dependency/stop-before/human-return/verify
        // skip), kept distinct from a non-green stop and a human-gate park, mirrors
        // the console panel's `skipped` tally.
        let issues_skipped = report
            .worked
            .iter()
            .filter(|r| r.outcome.is_none() && r.human_blockers.is_empty())
            .count() as u64;
        let u = &report.run_usage;
        info!(
            outcome = outcome_of(&report.stop),
            issues_done,
            issues_skipped,
            issues_total = queue.len() as u64,
            up = u.input,
            cr = u.cache_read,
            cw = u.cache_creation,
            out = u.output,
            duration_s = run_start.elapsed().as_secs(),
            "run finished"
        );
    }

    if let Some(notifier) = notifier.take() {
        notifier.shutdown();
    }
    // Tear down the CloudEvents sink alongside the notifier: the worker drains any
    // buffered events (including a final `run finished`) and POSTs them before the
    // process exits, joined under a bounded timeout so a wedged endpoint can't hold
    // the process open (ADR-0019).
    if let Some(events_handle) = events_handle.take() {
        events_handle.shutdown();
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
    // Issues stalled on a human gate in their path (ADR-0014) get their own
    // bucket and are kept out of the generic skipped tally, mirroring how the
    // live card gives them a distinct status.
    let hitl = report
        .worked
        .iter()
        .filter(|r| r.outcome.is_none() && !r.human_blockers.is_empty())
        .count() as u64;
    let skipped = report
        .worked
        .iter()
        .filter(|r| r.outcome.is_none() && r.human_blockers.is_empty())
        .count() as u64;

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
    let run_usage = &report.run_usage;
    let project_usage = ralphy_core::ledger::project_total(&slug);
    let to_lite = |u: &ralphy_core::Usage| ui::UsageLite {
        input: u.input,
        cache_read: u.cache_read,
        cache_creation: u.cache_creation,
        output: u.output,
        model: None,
    };

    // Read-time USD (ADR-0008 D8), priced per model and summed. The run total
    // prices `report.run_usage_by_model` (the runner's per-model split); the
    // project total groups the cumulative ledger rows by model and prices each.
    // USD never enters the ledger — re-pricing the table re-prices history.
    let price_table = pricing::PriceTable::load();
    let (run_usd, run_partial) = price_table.cost_usd_by_model(&report.run_usage_by_model);
    let mut project_by_model: std::collections::BTreeMap<String, ralphy_core::Usage> =
        std::collections::BTreeMap::new();
    for row in ralphy_core::read_project_rows(&slug) {
        project_by_model
            .entry(row.model.clone())
            .or_default()
            .add_tokens(&row.tokens);
    }
    let (project_usd, project_partial) = price_table.cost_usd_by_model(&project_by_model);

    let data = ui::PanelData {
        branch: report.branch,
        orig_branch: report.orig_branch,
        done,
        blocked: num_blocked,
        skipped,
        hitl,
        commits: report.commits,
        stop: panel_stop,
        branch_mode: panel_mode,
        dry_run: args.dry_run,
        undo_tag: report.undo_tag,
        run_breakdown: to_lite(run_usage),
        project_breakdown: to_lite(&project_usage),
        project_id: slug,
        run_usd,
        project_usd,
        run_usd_partial: run_partial,
        project_usd_partial: project_partial,
    };
    presenter.print_panel(&data);
    Ok(())
}

/// Force `stop_on_limit` for OpenCode runs only: OpenCode already self-waits short
/// limits and long ones carry no parseable reset, so auto-resume is never useful.
/// Claude and Codex pass the flag through unchanged — both emit a trustworthy reset
/// time (Codex an absolute RFC3339 instant, Claude a relative one), so both
/// auto-resume by default and honour `--stop-on-limit` as the opt-out.
/// Build a fully-configured adapter for one `CliAgent`, boxed as `&dyn Agent`.
/// Centralizes the per-vendor construction the composition root needs once for
/// the executor and (only in a split run) once for the planner — so `--plan-agent`
/// can wire two adapters without duplicating the match. The `String`/`Option`
/// config values are cloned per call so the same `RunArgs` can back both builds.
fn build_agent(
    which: CliAgent,
    args: &RunArgs,
    run_dir: PathBuf,
    run_deadline: Option<std::time::Instant>,
    persisted_opencode_model: Option<String>,
    claude: &ResolvedClaude,
) -> Box<dyn Agent> {
    match which {
        CliAgent::Claude => Box::new(
            ClaudeAgent::new(
                non_empty(claude.plan_model.clone()),
                non_empty(claude.plan_effort.clone()),
                run_dir,
            )
            .with_exec_config(
                non_empty(args.exec_model.clone().unwrap_or_default()),
                non_empty(claude.exec_effort.clone()),
                claude.default_exec_model.clone(),
                claude.max_minutes_per_issue,
                !args.no_remote_control,
                args.headless_exec,
                args.max_exec_calls,
            )
            .with_run_deadline(run_deadline),
        ),
        CliAgent::Codex => Box::new(
            CodexAgent::new(
                non_empty(args.exec_model.clone().unwrap_or_default()),
                run_dir,
            )
            .with_run_deadline(run_deadline)
            .with_max_minutes_per_issue(claude.max_minutes_per_issue),
        ),
        CliAgent::OpenCode => Box::new(
            OpenCodeAgent::new(
                config::resolve_opencode_model(args.exec_model.clone(), persisted_opencode_model),
                run_dir,
            )
            .with_variant(non_empty(args.exec_variant.clone().unwrap_or_default()))
            .with_run_deadline(run_deadline)
            .with_max_minutes_per_issue(claude.max_minutes_per_issue),
        ),
    }
}

/// Resolve which adapter plans: the explicit `--plan-agent`, or `--agent` when
/// the flag is omitted. An absent flag MUST equal `--agent` so a single-agent run
/// is unchanged (docs/adr/0009).
fn resolve_plan_agent(plan_agent: Option<CliAgent>, agent: CliAgent) -> CliAgent {
    plan_agent.unwrap_or(agent)
}

/// Remove `RALPHY_EVENTS_TOKEN` from the process environment so no spawned child
/// (adapter/agent) inherits the sink's bearer token (ADR-0019). Called once at boot
/// after the effective token is resolved and captured by the sink transport — the
/// run keeps using it, children never see it. Mirrors the `ANTHROPIC_API_KEY` scrub.
fn strip_events_token_from_env() {
    std::env::remove_var(events::config::TOKEN_ENV);
}

/// Map a queue's [`StopReason`] to the `run.finished` `outcome` label (ADR-0019).
/// `None` (the whole queue was worked) is `completed`; a usage-limit stop has no
/// `outcome` value in the contract enum, so it collapses to `non_green` — a
/// usage-limit stop is a non-clean completion (docs/events.md `run.finished`).
fn outcome_of(stop: &Option<StopReason>) -> &'static str {
    match stop {
        None => "completed",
        Some(StopReason::NonGreen { .. }) => "non_green",
        Some(StopReason::Deadline) => "deadline",
        Some(StopReason::StopBefore { .. }) => "stop_before",
        Some(StopReason::Limit { .. }) => "non_green",
    }
}

/// The operating run branch commits land on, for the `data.git.branch` block
/// (ADR-0019 amendment #96): a fresh `afk/run-<stamp>` in `new` mode (matching the
/// `afk/run-{stamp}` format the runner cuts), or the current branch in `current`
/// mode (empty when the current branch could not be resolved). Resolved before the
/// events ctx so `data.git` is constant from the first event.
fn operating_branch(mode: BranchMode, stamp: &str, start_branch: Option<&str>) -> String {
    match mode {
        BranchMode::New => format!("afk/run-{stamp}"),
        BranchMode::Current => start_branch.unwrap_or_default().to_string(),
    }
}

/// Build the human-readable scope phrase for the "No open issues for …" notice.
/// An explicit `--issues` selection or `--only-issue` names the numbers; a label
/// queue names the labels and, when an assignee filter is active, appends
/// `assigned to <login>` so the empty notice reveals the filter (ADR-0021,
/// criterion 7). `--only-issue`/`--issues` bypass the filter, so `assignee` is
/// only ever appended on the labels path.
fn empty_queue_scope(
    issues: &[u64],
    only_issue: Option<u64>,
    labels: &[String],
    assignee: Option<&str>,
) -> String {
    if !issues.is_empty() {
        let list = issues
            .iter()
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("issues [{list}]");
    }
    match only_issue {
        Some(n) => format!("issue #{n}"),
        None => {
            let base = format!("labels [{}]", labels.join(", "));
            match assignee {
                Some(a) => format!("{base} assigned to {a}"),
                None => base,
            }
        }
    }
}

/// Pure predicate layer: returns `Err(message)` for the first agent whose
/// `cli_name()` the `locate` closure reports absent, else `Ok(())`. The
/// `locate` indirection lets unit tests inject a fake resolver with no PATH
/// dependency.
fn check_agents_present(
    executor: CliAgent,
    planner: CliAgent,
    locate: impl Fn(&str) -> bool,
) -> Result<(), String> {
    for which in [executor, planner] {
        let cli = which.cli_name();
        if !locate(cli) {
            return Err(format!(
                "the `{cli}` CLI was not found on PATH, PATHEXT, or ~/.local/bin. \
                Install it, or select another agent with --agent / --plan-agent."
            ));
        }
    }
    Ok(())
}

/// Thin wrapper that wires `check_agents_present` to the real `locate_program`
/// resolver and maps the string error into `anyhow`.
fn preflight_agents(executor: CliAgent, planner: CliAgent) -> Result<()> {
    check_agents_present(executor, planner, |n| {
        ralphy_adapter_support::locate_program(n).is_some()
    })
    .map_err(|e| anyhow::anyhow!(e))
}

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
    events: Option<events::sink::EventsLayer>,
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
        .with(notifier)
        .with(events);

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
    fn strip_events_token_removes_env_var() {
        // Guard the process-global env var against the other events-store tests.
        let _g = events::config::ENV_LOCK.lock().unwrap();
        std::env::set_var(events::config::TOKEN_ENV, "sekret");
        assert!(std::env::var(events::config::TOKEN_ENV).is_ok());
        strip_events_token_from_env();
        assert!(
            std::env::var(events::config::TOKEN_ENV).is_err(),
            "token must be absent after strip"
        );
    }

    #[test]
    fn operating_branch_derives_per_mode() {
        // `new` mode cuts a fresh `afk/run-<stamp>` regardless of the current branch.
        assert_eq!(
            operating_branch(BranchMode::New, "20260703-120000", Some("feature")),
            "afk/run-20260703-120000"
        );
        // `current` mode reports the current branch verbatim.
        assert_eq!(
            operating_branch(BranchMode::Current, "20260703-120000", Some("feature")),
            "feature"
        );
        // `current` mode with no resolvable current branch degrades to empty.
        assert_eq!(
            operating_branch(BranchMode::Current, "20260703-120000", None),
            ""
        );
    }

    #[test]
    fn outcome_of_maps_every_stop_reason() {
        assert_eq!(outcome_of(&None), "completed");
        assert_eq!(
            outcome_of(&Some(StopReason::NonGreen {
                number: 1,
                outcome: Outcome::Stuck,
            })),
            "non_green"
        );
        assert_eq!(outcome_of(&Some(StopReason::Deadline)), "deadline");
        assert_eq!(
            outcome_of(&Some(StopReason::StopBefore { number: 2 })),
            "stop_before"
        );
        // A usage-limit stop has no `outcome` value in the contract enum, so it
        // collapses to non_green (a non-clean completion).
        assert_eq!(
            outcome_of(&Some(StopReason::Limit {
                number: 3,
                reset: Some("14:30".into()),
            })),
            "non_green"
        );
    }

    #[test]
    fn init_subcommand_is_registered() {
        use clap::CommandFactory;
        assert!(
            Cli::command()
                .get_subcommands()
                .any(|s| s.get_name() == "init"),
            "the `init` subcommand must be registered in the CLI"
        );
    }

    #[test]
    fn triage_subcommand_is_registered() {
        use clap::CommandFactory;
        assert!(
            Cli::command()
                .get_subcommands()
                .any(|s| s.get_name() == "triage"),
            "the `triage` subcommand must be registered in the CLI"
        );
    }

    #[test]
    fn queue_label_is_repeatable_and_preserves_order() {
        // The resolver (`resolve_queue_labels`) treats a non-empty explicit set as
        // a full replacement; this guards the CLI seam that feeds it — multiple
        // `--queue-label` flags must arrive intact and in order, and an absent flag
        // must yield an empty vec so the defaults path is taken.
        let cli = Cli::try_parse_from([
            "ralphy",
            "run",
            "--queue-label",
            "foo",
            "--queue-label",
            "bar",
        ])
        .expect("run with repeated --queue-label must parse");
        let Command::Run(args) = cli.command else {
            panic!("expected the `run` subcommand");
        };
        assert_eq!(args.queue_label, vec!["foo", "bar"]);

        let cli = Cli::try_parse_from(["ralphy", "run"]).expect("bare run must parse");
        let Command::Run(args) = cli.command else {
            panic!("expected the `run` subcommand");
        };
        assert!(
            args.queue_label.is_empty(),
            "no --queue-label must leave the set empty so defaults apply"
        );
    }

    #[test]
    fn if_idle_flag_parses_and_defaults_off() {
        let cli = Cli::try_parse_from(["ralphy", "run", "--if-idle"])
            .expect("run with --if-idle must parse");
        let Command::Run(args) = cli.command else {
            panic!("expected the `run` subcommand");
        };
        assert!(args.if_idle);

        let cli = Cli::try_parse_from(["ralphy", "run"]).expect("bare run must parse");
        let Command::Run(args) = cli.command else {
            panic!("expected the `run` subcommand");
        };
        assert!(!args.if_idle, "--if-idle must default to off");
    }

    #[test]
    fn assignee_flags_parse_and_conflict() {
        // `--assignee` captures the login.
        let cli = Cli::try_parse_from(["ralphy", "run", "--assignee", "@me"])
            .expect("run with --assignee must parse");
        let Command::Run(args) = cli.command else {
            panic!("expected the `run` subcommand");
        };
        assert_eq!(args.assignee.as_deref(), Some("@me"));
        assert!(!args.no_assignee);

        // `--no-assignee` alone parses and defaults `assignee` to None.
        let cli = Cli::try_parse_from(["ralphy", "run", "--no-assignee"])
            .expect("run with --no-assignee must parse");
        let Command::Run(args) = cli.command else {
            panic!("expected the `run` subcommand");
        };
        assert!(args.no_assignee);
        assert_eq!(args.assignee, None);

        // The two are mutually exclusive — clap rejects both together.
        assert!(
            Cli::try_parse_from(["ralphy", "run", "--assignee", "@me", "--no-assignee"]).is_err(),
            "--assignee and --no-assignee must conflict"
        );
    }

    #[test]
    fn empty_queue_scope_names_the_filter() {
        // Active filter on a label queue names the assignee.
        let scope = empty_queue_scope(&[], None, &["ready-for-agent".to_string()], Some("@me"));
        assert!(scope.contains("@me"), "scope must name the filter: {scope}");
        assert!(scope.contains("assigned to"), "got: {scope}");

        // No filter omits the "assigned to" phrase.
        let scope = empty_queue_scope(&[], None, &["ready-for-agent".to_string()], None);
        assert!(
            !scope.contains("assigned to"),
            "unfiltered scope must not mention an assignee: {scope}"
        );

        // Explicit selections never carry the filter phrase.
        let scope = empty_queue_scope(&[5, 3], None, &[], None);
        assert_eq!(scope, "issues [#5, #3]");
        let scope = empty_queue_scope(&[], Some(7), &["ready-for-agent".to_string()], None);
        assert_eq!(scope, "issue #7");
    }

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

    #[test]
    fn resolution_byte_for_byte_when_absent() {
        // With no flag AND no setting, every knob must resolve to today's
        // hardcoded default, leaving behaviour unchanged (ADR-0010).
        assert_eq!(
            config::resolve_str(None, None, "origin/main"),
            "origin/main"
        );
        assert_eq!(config::resolve_str(None, None, "opus"), "opus");
        assert_eq!(config::resolve_str(None, None, "medium"), "medium");
        assert_eq!(config::resolve_str(None, None, "sonnet"), "sonnet");
        assert_eq!(config::resolve_u64(None, None, 90), 90);

        // The branch_mode resolution chain with (no flag, no setting) yields New.
        let flag: Option<CliBranchMode> = None;
        let persisted: Option<String> = None;
        let branch_mode: BranchMode = flag
            .map(BranchMode::from)
            .or_else(|| {
                persisted
                    .as_deref()
                    .and_then(|m| config::parse_branch_mode(m).ok())
            })
            .unwrap_or(BranchMode::New);
        assert_eq!(branch_mode, BranchMode::New);
    }

    #[test]
    fn plan_agent_defaults_to_the_executor_when_omitted() {
        // Omitted `--plan-agent` resolves to `--agent`, keeping single-agent runs
        // unchanged; an explicit flag overrides it (any combination allowed).
        assert_eq!(
            resolve_plan_agent(None, CliAgent::Claude),
            CliAgent::Claude,
            "absent flag equals --agent"
        );
        assert_eq!(
            resolve_plan_agent(Some(CliAgent::Claude), CliAgent::OpenCode),
            CliAgent::Claude,
            "explicit --plan-agent overrides --agent"
        );
    }

    #[test]
    fn check_agents_present_aborts_when_executor_absent() {
        let result = check_agents_present(CliAgent::Claude, CliAgent::Claude, |_| false);
        let err = result.unwrap_err();
        assert!(
            err.contains("claude"),
            "message must name the missing cli: {err}"
        );
        assert!(
            err.contains("--agent"),
            "message must mention --agent: {err}"
        );
        assert!(
            err.contains("--plan-agent"),
            "message must mention --plan-agent: {err}"
        );
    }

    #[test]
    fn check_agents_present_gates_planner() {
        // executor (Claude) is present; planner (Codex) is absent → Err naming codex.
        let result = check_agents_present(CliAgent::Claude, CliAgent::Codex, |n| n == "claude");
        let err = result.unwrap_err();
        assert!(
            err.contains("codex"),
            "message must name the absent planner: {err}"
        );
    }

    #[test]
    fn check_agents_present_ok_when_all_present() {
        let result = check_agents_present(CliAgent::Claude, CliAgent::Codex, |_| true);
        assert!(result.is_ok());
    }
}
