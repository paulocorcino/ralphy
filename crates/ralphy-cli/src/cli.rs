//! The clap CLI surface: the top-level `Cli`/`Command`, the `run`/`consolidate`
//! argument structs, the internal `Hook` subcommands, and the CLI-only
//! agent/branch-mode selector enums. Kept as a CLI concern separate from the
//! composition root (`main.rs`) and the run orchestrator (`run.rs`); the enums map
//! into the core's own types at the boundary (docs/adr/0004, docs/adr/0002).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use ralphy_core::{BranchMode, Effort};

use crate::{
    blob, changes, config, daemon, init, install, issues, models, mutate, schedule, stop, sync,
    telegram, triage, update, usage,
};

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
pub(crate) struct Cli {
    /// Print the git-published version and exit.
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: (),

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
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
    /// Register / inspect / remove a native OS timer that re-invokes `ralphy
    /// run` on a cadence (Windows Task Scheduler or cron) — ADR-0026.
    #[command(subcommand)]
    Schedule(schedule::ScheduleCommand),
    /// Run the resident daemon in the foreground: a localhost HTTP listener
    /// serving the embedded workbench UI (docs/adr/0032). Ctrl+C stops it.
    Daemon(daemon::DaemonArgs),
    /// Run-lock-aware git branch ops (ADR-0036 §6).
    #[command(subcommand)]
    Branch(mutate::BranchCommand),
    /// Workbench worktrees under `.ralphy/worktrees/` (ADR-0063).
    #[command(subcommand)]
    Worktree(mutate::WorktreeCommand),
    /// Run-lock-aware label mutation (ADR-0036 §6).
    #[command(subcommand)]
    Label(mutate::LabelCommand),
    /// Read-only working-tree change set of a repo.
    #[command(subcommand)]
    Changes(changes::ChangesCommand),
    /// Read-only file content at a git revision (the diff's original side).
    #[command(subcommand)]
    Blob(blob::BlobCommand),
    /// The branch's upstream state, plus an operator-triggered fetch and a
    /// fast-forward-only pull (ADR-0036 §6).
    #[command(subcommand)]
    Sync(sync::SyncCommand),
    /// Ask a live run in this repo to stop (docs/adr/0054). Writes a request the
    /// run itself acts on — nothing here kills a process.
    ///
    /// Not to be confused with the internal `ralphy hook stop`, which is the
    /// agent's session-exit hook and has nothing to do with runs.
    Stop(stop::StopArgs),
    /// Report what has been published and where this build stands against it
    /// (`--check`), on the `rc` or `stable` channel (docs/adr/0056).
    Update(update::UpdateArgs),
}

#[derive(Subcommand)]
pub(crate) enum HookCommand {
    /// Stop hook: record the session's exit sentinel to `$RALPHY_FLAG_FILE`.
    Stop,
    /// PreToolUse guard: block destructive commands/writes.
    Guard,
    /// PostToolUse (Bash): record measured verify-command durations for the
    /// verification-cost gate.
    Post,
    /// Agent-state hook (ADR-0059): append the hook event to
    /// `$RALPHY_STATUS_FILE` for the adapter or the daemon to fold. Prints
    /// `{}` and exits 0 whatever happens.
    Status,
}

#[derive(Args)]
pub(crate) struct RunArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// Which agent CLI executes the run: `claude` (the default, a live PTY
    /// session) or one of the headless adapters — `codex`, `copilot`, `kimi`,
    /// `opencode`. Selects the
    /// executor; pair with `--plan-agent` to plan with a different adapter.
    /// Selected per run; the core never learns which vendor it holds.
    #[arg(long = "agent", value_enum, default_value_t = CliAgent::Claude)]
    pub(crate) agent: CliAgent,

    /// Adapter for the planning phase; defaults to `--agent` when omitted, so a
    /// single-agent run is unchanged. The canonical split is
    /// `--agent opencode --plan-agent claude` (Claude plans, OpenCode executes).
    /// Any planner/executor combination is accepted (ADR-0009).
    #[arg(long = "plan-agent", value_enum)]
    pub(crate) plan_agent: Option<CliAgent>,

    /// Work only this issue number (filters the queue to it). Omit to work the
    /// whole queue.
    #[arg(long)]
    pub(crate) only_issue: Option<u64>,

    /// Work exactly these issues, in the order given, ignoring queue labels:
    /// `--issues 5,3,9`. Each number is fetched directly (no label filter, no
    /// dependency re-ordering), so the run drains the list as a sequence. Like
    /// `--only-issue`, a `stop-before` label on a listed issue is ignored;
    /// unlike it, human-return labels (ADR-0016) are still respected. Mutually
    /// exclusive with `--only-issue`.
    #[arg(long, value_delimiter = ',', conflicts_with = "only_issue")]
    pub(crate) issues: Vec<u64>,

    /// Queue label(s): an open issue carrying ANY of these is worked. Repeatable.
    /// When omitted, defaults to ["ready-for-agent", "AFK"] plus any label
    /// mapped from "ready-for-agent" in docs/agents/triage-labels.md.
    #[arg(long = "queue-label")]
    pub(crate) queue_label: Vec<String>,

    /// Global wall-clock budget (hours): don't start a new issue past it. Omit
    /// for no deadline.
    #[arg(long)]
    pub(crate) deadline_hours: Option<f64>,

    /// Plan only; make no source changes and drop the empty run branch.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Drop the animated presenter and print raw INFO `tracing` lines instead
    /// (also engaged by `RUST_LOG`/`RALPHY_LOG`). Useful for debugging or CI.
    #[arg(long)]
    pub(crate) verbose: bool,

    /// Commit-ish the run branch is cut from. Only used with `--branch-mode new`
    /// (default: origin/main, or `base_branch` in settings.json).
    #[arg(long)]
    pub(crate) base_branch: Option<String>,

    /// Where commits land: `new` cuts a fresh `afk/run-*` branch off
    /// `--base-branch`; `current` commits straight onto the branch the repo is
    /// already on (no new branch, `--base-branch` ignored). (default: new, or
    /// `branch_mode` in settings.json).
    #[arg(long = "branch-mode", value_enum)]
    pub(crate) branch_mode: Option<CliBranchMode>,

    /// Planning model (default: opus, or `claude.plan_model` in settings.json;
    /// for `--agent copilot` / a Copilot `--plan-agent`, the persisted fallback
    /// is `copilot.plan_model` instead, and an unset value omits `--model`).
    #[arg(long)]
    pub(crate) plan_model: Option<String>,

    /// Vendor-neutral planning effort.
    #[arg(long)]
    pub(crate) plan_effort: Option<Effort>,

    /// Force the execution model for the issue (overrides the plan's judgment;
    /// for `--agent copilot`, the persisted fallback is `copilot.exec_model`
    /// instead, and an unset value omits `--model`).
    #[arg(long)]
    pub(crate) exec_model: Option<String>,

    /// OpenCode provider-native `--variant` dialect passed through to
    /// `opencode run`. Orthogonal to `--plan-effort`/`--exec-effort` (those are
    /// documented no-ops for this agent; docs/adr/0005 D3 amendment / ADR-0044
    /// D8). Omitted when unset so the adapter never sends a value the provider
    /// rejects. Only used by `--agent opencode`.
    #[arg(long)]
    pub(crate) exec_variant: Option<String>,

    /// Vendor-neutral execution effort.
    #[arg(long)]
    pub(crate) exec_effort: Option<Effort>,

    /// Execution model used when the plan emits no complexity judgment
    /// (default: sonnet, or `claude.default_exec_model` in settings.json).
    #[arg(long)]
    pub(crate) default_exec_model: Option<String>,

    /// Opt-in per-issue wall-clock cap (minutes) before the session is
    /// reclaimed (default: no cap, or `claude.max_minutes_per_issue` in
    /// settings.json). With no cap an issue is bounded only by
    /// `--deadline-hours`; a wedged child is caught by `--idle-minutes`.
    #[arg(long)]
    pub(crate) max_minutes_per_issue: Option<u64>,

    /// Reap a child that has made no progress for this many minutes — the
    /// liveness net that catches a wedged or silently-retrying agent (default:
    /// 20 headless / 45 interactive, or `idle_minutes` in settings.json). Pass
    /// `0` to disable it.
    #[arg(long)]
    pub(crate) idle_minutes: Option<u64>,

    /// Enable Remote Control so you can follow/intervene from the mobile app.
    #[arg(long, overrides_with = "no_remote_control")]
    pub(crate) remote_control: bool,

    /// Disable Remote Control for the execution session.
    #[arg(long = "no-remote-control", overrides_with = "remote_control")]
    pub(crate) no_remote_control: bool,

    /// Use a `claude -p` loop instead of an interactive PTY session (for
    /// environments with no TTY, e.g. CI).
    #[arg(long)]
    pub(crate) headless_exec: bool,

    /// Maximum number of `claude -p` calls per issue before declaring stuck
    /// (headless mode only).
    #[arg(long, default_value_t = 6)]
    pub(crate) max_exec_calls: u32,

    /// On a usage limit, stop and report the reset instead of the default
    /// (wait for the reset and auto-resume the same issue). See docs/adr/0003.
    #[arg(long)]
    pub(crate) stop_on_limit: bool,

    /// Mute the Telegram run notifier for this run (no card, no pushes), even
    /// when Telegram is configured. See docs/adr/0007.
    #[arg(long)]
    pub(crate) no_telegram: bool,

    /// Override the auto-derived Telegram card title for this run.
    #[arg(long)]
    pub(crate) title: Option<String>,

    /// Skip this invocation (exit 0) when another run is already active in the
    /// repo — the anti-overlap flag scheduled invocations pass so a timer never
    /// piles a run onto a live one. Without it a live run only warns.
    #[arg(long)]
    pub(crate) if_idle: bool,

    /// Build the label queue only from issues this login is among the assignees
    /// of (`gh --assignee` semantics; `@me` = the authenticated user). Overrides a
    /// persisted `queue.assignee`. `--only-issue`/`--issues` ignore this filter.
    #[arg(long)]
    pub(crate) assignee: Option<String>,

    /// Disable a persisted `queue.assignee` filter for this one invocation, so the
    /// queue is built unfiltered. Mutually exclusive with `--assignee`.
    #[arg(long = "no-assignee", conflicts_with = "assignee")]
    pub(crate) no_assignee: bool,
}

#[derive(Args)]
pub(crate) struct ConsolidateArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// Which agent CLI drives the consolidation session (docs/adr/0031). Defaults
    /// to Claude so a bare `ralphy consolidate` is unchanged; every adapter drives
    /// the same charter.
    #[arg(long = "agent", value_enum, default_value_t = CliAgent::Claude)]
    pub(crate) agent: CliAgent,

    /// Model for the consolidation session. Curation is judgment-heavy (dedup,
    /// conflict resolution, what to cut); when omitted the default is the agent's —
    /// opus for Claude, the adapter's own default for the rest.
    #[arg(long)]
    pub(crate) model: Option<String>,

    /// Reasoning effort for the consolidation session (Claude/Codex only; Kimi and
    /// OpenCode have no such knob). When omitted, `medium` on Claude.
    #[arg(long)]
    pub(crate) effort: Option<String>,

    /// Wall-clock budget (minutes) before the session is reclaimed.
    #[arg(long, default_value_t = 30)]
    pub(crate) max_minutes: u64,
}

/// The CLI's own agent-selector enum so `clap` stays a CLI concern; the composition
/// root maps it to the boxed `&dyn Agent` it hands the core (docs/adr/0004 D1).
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliAgent {
    Claude,
    Codex,
    // One word `copilot` derives correctly from the variant name — no `#[value]` attr.
    Copilot,
    // One word `cursor` derives correctly from the variant name — no `#[value]` attr.
    // The binary is `cursor-agent`/`agent` (ADR-0042 D14); the SELECTOR is the
    // vendor's name, as with every other adapter.
    Cursor,
    // One word `gemini` derives correctly from the variant name — no `#[value]` attr.
    Gemini,
    // One word `kimi` derives correctly from the variant name — no `#[value]` attr.
    Kimi,
    // The ADR-0005 contract and the documented invocation are `--agent opencode`
    // (one word). Clap would otherwise derive the kebab-cased `open-code` from the
    // variant name; pin the spelling and keep that derivation as an alias.
    #[value(name = "opencode", alias = "open-code")]
    OpenCode,
}

impl CliAgent {
    pub(crate) fn cli_name(self) -> &'static str {
        match self {
            CliAgent::Claude => "claude",
            CliAgent::Codex => "codex",
            CliAgent::Copilot => "copilot",
            CliAgent::Cursor => "cursor",
            CliAgent::Gemini => "gemini",
            CliAgent::Kimi => "kimi",
            CliAgent::OpenCode => "opencode",
        }
    }
}

/// The CLI's own branch-mode enum so `clap` stays a CLI concern; it converts into
/// the core's `BranchMode` (see docs/adr/0002).
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliBranchMode {
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

#[cfg(test)]
mod tests;
