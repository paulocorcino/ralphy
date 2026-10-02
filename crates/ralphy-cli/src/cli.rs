//! The clap CLI surface: the top-level `Cli`/`Command`, the `run`/`consolidate`
//! argument structs, the internal `Hook` subcommands, and the CLI-only
//! agent/branch-mode selector enums. Kept as a CLI concern separate from the
//! composition root (`main.rs`) and the run orchestrator (`run.rs`); the enums map
//! into the core's own types at the boundary (docs/adr/0004, docs/adr/0002).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use ralphy_core::{BranchMode, Effort};

use crate::{
    blob, changes, config, daemon, gemini, host, init, install, issues, models, mutate, schedule,
    stop, sync, telegram, triage, update, usage,
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
    /// Print the version and exit.
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: (),

    #[command(subcommand)]
    pub(crate) command: Command,
}

/// The top-level commands, in the order `--help` lists them.
#[derive(Subcommand)]
pub(crate) enum Command {
    // Getting started.
    /// Make `ralphy` a command you can run from any folder.
    ///
    /// It links (or copies) this program into a folder on your PATH.
    Install(install::InstallArgs),
    /// Check that a repo is ready for Ralphy.
    ///
    /// It checks for Python, a `gh` login, a GitHub remote, and at least one
    /// agent CLI you are logged in to.
    // ADR-0012 stage 1.
    Init(init::InitArgs),
    /// Start the daemon that serves the workbench to your browser.
    ///
    /// It runs in this terminal. Press Ctrl+C to stop it.
    // ADR-0032.
    Daemon(daemon::DaemonArgs),
    /// Add, check, or remove another computer that the workbench reaches over
    /// SSH.
    #[command(subcommand)]
    Host(host::HostCommand),

    // Working the queue.
    /// Let an agent work through the repo's issue queue.
    Run(Box<RunArgs>),
    /// Ask the run in this repo to stop.
    ///
    /// The run reads the request and stops by itself; this command does not
    /// kill any process.
    // ADR-0054.
    Stop(stop::StopArgs),
    /// List the open issues, or show one issue in detail.
    ///
    /// The list shows what a run would do with each issue; `issues show <n>`
    /// shows one issue. Changes nothing. Use `--format json` or `--fields` for
    /// output that a script reads.
    // ADR-0020.
    Issues(issues::IssuesArgs),
    /// Let an agent sort the issues labeled `triage-agent`.
    ///
    /// Each one is promoted (made ready for an agent), consolidated (given one
    /// spec comment), or bounced (sent back to its author). You see a preview
    /// before anything changes; `--yes` skips it, for scheduled runs.
    // ADR-0017.
    Triage(triage::TriageArgs),
    /// Start `ralphy run` or `ralphy triage` on a timer.
    ///
    /// The operating system starts them (Windows Task Scheduler or cron). Add,
    /// show, or remove the timers.
    // ADR-0026.
    #[command(subcommand)]
    Schedule(schedule::ScheduleCommand),
    /// Merge the notes that runs leave into one `KNOWLEDGE.md`.
    ///
    /// The notes are in `.ralphy/knowledge/issue-<N>.md`. An agent merges them,
    /// removes repeated points, and checks them; the used notes move to
    /// `knowledge/raw/`.
    Consolidate(ConsolidateArgs),
    /// List, add, or remove worktrees (extra checkouts of the repo).
    ///
    /// The workbench keeps them under `.ralphy/worktrees/`.
    // ADR-0063.
    #[command(subcommand)]
    Worktree(mutate::WorktreeCommand),

    // Settings and reports.
    /// Change the Ralphy settings of this repo.
    ///
    /// For example `ralphy config set opencode.model <model>`.
    Config(config::ConfigArgs),
    /// Set up the optional Telegram messages about runs.
    #[command(subcommand)]
    Telegram(telegram::TelegramCommand),
    /// List the models an agent can use.
    ///
    /// The default agent is opencode. Ralphy runs the agent's own command to
    /// list them.
    Models(models::ModelsArgs),
    /// Show the tokens the agents used in this project, and what they cost.
    ///
    /// Group the rows with `--by`, filter them with `--since`, or export them
    /// with `--format csv|json`. Costs use the prices of today; they are not
    /// saved.
    Usage(usage::UsageArgs),
    /// Update Ralphy to the newest release.
    ///
    /// It follows the `rc` or the `stable` channel. With `--check`, it only
    /// shows what is published.
    // ADR-0056.
    Update(update::UpdateArgs),

    // Run by the workbench and the agents; listed apart by `command()`.
    /// Hooks that the agent calls during a run.
    #[command(subcommand, hide = true)]
    Hook(HookCommand),
    /// List, create, or switch git branches.
    // ADR-0036 §6.
    #[command(subcommand, hide = true)]
    Branch(mutate::BranchCommand),
    /// Add or remove labels on an issue.
    // ADR-0036 §6.
    #[command(subcommand, hide = true)]
    Label(mutate::LabelCommand),
    /// List, stage, commit, or discard the changes in a repo.
    #[command(subcommand, hide = true)]
    Changes(changes::ChangesCommand),
    /// Print a file as it is in the last commit.
    #[command(subcommand, hide = true)]
    Blob(blob::BlobCommand),
    /// Compare the branch with its remote, fetch, pull, or push.
    // ADR-0036 §6.
    #[command(subcommand, hide = true)]
    Sync(sync::SyncCommand),
    /// Prepare Gemini's own configuration in a repo.
    // ADR-0040 Amendment 3.
    #[command(subcommand, hide = true)]
    Gemini(gemini::GeminiCommand),
}

/// The CLI as `main` parses it. The hidden commands are the ones the workbench
/// and the agents run; `--help` lists them after the others, under their own
/// heading. The list is built from their own summaries, so it cannot drift.
pub(crate) fn command() -> clap::Command {
    use clap::CommandFactory;

    let cmd = Cli::command();
    let hidden: Vec<(String, String)> = cmd
        .get_subcommands()
        .filter(|sub| sub.is_hide_set())
        .map(|sub| {
            let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
            (sub.get_name().to_string(), about)
        })
        .collect();
    let width = hidden.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    let header = cmd.get_styles().get_header();
    let mut text = format!(
        "{}Run for you by the workbench and the agents:{}\n",
        header.render(),
        header.render_reset()
    );
    for (name, about) in &hidden {
        text.push_str(&format!("  {name:width$}  {about}\n"));
    }
    cmd.after_help(text.trim_end().to_string())
}

#[derive(Subcommand)]
pub(crate) enum HookCommand {
    /// Record in `$RALPHY_FLAG_FILE` that the agent session ended.
    Stop,
    /// Block commands and file writes that would destroy work.
    Guard {
        /// Also refuse a check command already measured as slow while the plan
        /// still has open steps. Execute sessions only.
        #[arg(long)]
        cost_gate: bool,
    },
    /// Record how long each check command took.
    Post,
    /// Record what the agent is doing in `$RALPHY_STATUS_FILE`, so the workbench
    /// can show it. Always prints `{}` and exits 0.
    // ADR-0059.
    Status,
}

#[derive(Args)]
pub(crate) struct RunArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// The agent that does the work. `claude` (the default) runs in a live
    /// terminal session; the other agents run without one. Use `--plan-agent`
    /// to plan with a different agent.
    #[arg(long = "agent", value_enum, default_value_t = CliAgent::Claude)]
    pub(crate) agent: CliAgent,

    /// The agent that writes the plan. Without it, `--agent` also plans. For
    /// example, `--agent opencode --plan-agent claude` means Claude plans and
    /// OpenCode does the work. Any two agents can be paired.
    // ADR-0009.
    #[arg(long = "plan-agent", value_enum)]
    pub(crate) plan_agent: Option<CliAgent>,

    /// Work only this issue. Without it, the whole queue is worked.
    #[arg(long)]
    pub(crate) only_issue: Option<u64>,

    /// Work exactly these issues, in this order, whatever their queue labels:
    /// `--issues 5,3,9`. Dependencies do not change the order. As with
    /// `--only-issue`, a `stop-before` label is ignored. Unlike `--only-issue`,
    /// an issue with a label that returns it to a person is skipped. Cannot be
    /// used with `--only-issue`.
    // ADR-0016.
    #[arg(long, value_delimiter = ',', conflicts_with = "only_issue")]
    pub(crate) issues: Vec<u64>,

    /// A label that puts an open issue in the queue (repeat for more). Default:
    /// `ready-for-agent` and `AFK`, plus any label mapped to `ready-for-agent`
    /// in docs/agents/triage-labels.md.
    #[arg(long = "queue-label")]
    pub(crate) queue_label: Vec<String>,

    /// Start no new issue after this many hours. Default: no limit.
    #[arg(long)]
    pub(crate) deadline_hours: Option<f64>,

    /// Only write the plans: change no files, and delete the empty run branch.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Print plain log lines instead of the animated display (setting
    /// `RUST_LOG` or `RALPHY_LOG` does the same). Useful for debugging or CI.
    #[arg(long)]
    pub(crate) verbose: bool,

    /// The branch or commit that the run branch starts from. Only used with
    /// `--branch-mode new`. Default: origin/main, or `base_branch` in
    /// settings.json.
    #[arg(long)]
    pub(crate) base_branch: Option<String>,

    /// Where the commits go: `new` makes a new `afk/run-*` branch from
    /// `--base-branch`; `current` commits on the branch you are on, and ignores
    /// `--base-branch`. Default: new, or `branch_mode` in settings.json.
    #[arg(long = "branch-mode", value_enum)]
    pub(crate) branch_mode: Option<CliBranchMode>,

    /// The model that writes the plan. Default: opus, or `claude.plan_model` in
    /// settings.json. For Copilot the setting is `copilot.plan_model`; when it
    /// is not set, Copilot chooses its own model.
    #[arg(long)]
    pub(crate) plan_model: Option<String>,

    /// Reasoning effort for the plan. The same values work for every agent.
    #[arg(long)]
    pub(crate) plan_effort: Option<Effort>,

    /// Use this model for the work on every issue, instead of the model the
    /// plan chooses. For Copilot the setting is `copilot.exec_model`; when it
    /// is not set, Copilot chooses its own model.
    #[arg(long)]
    pub(crate) exec_model: Option<String>,

    /// OpenCode only: a `--variant` value, passed to `opencode run` as it is.
    /// `--plan-effort` and `--exec-effort` have no effect on OpenCode. When
    /// unset, Ralphy sends no variant.
    // ADR-0005 D3 amendment, ADR-0044 D8.
    #[arg(long)]
    pub(crate) exec_variant: Option<String>,

    /// Reasoning effort for the work. The same values work for every agent.
    #[arg(long)]
    pub(crate) exec_effort: Option<Effort>,

    /// The model for the work when the plan does not choose one. Default:
    /// sonnet, or `claude.default_exec_model` in settings.json.
    #[arg(long)]
    pub(crate) default_exec_model: Option<String>,

    /// Stop the work on an issue after this many minutes. Default: no limit, or
    /// `claude.max_minutes_per_issue` in settings.json. Without a limit, only
    /// `--deadline-hours` and `--idle-minutes` stop an issue.
    #[arg(long)]
    pub(crate) max_minutes_per_issue: Option<u64>,

    /// Stop an agent that shows no progress for this many minutes, for example
    /// one that is stuck or that retries without saying so. Default: 20 without
    /// a terminal session, 45 with one, or `idle_minutes` in settings.json. `0`
    /// turns this off.
    #[arg(long)]
    pub(crate) idle_minutes: Option<u64>,

    /// Turn on Claude's Remote Control, so you can follow the work and step in
    /// from the mobile app.
    #[arg(long, overrides_with = "no_remote_control")]
    pub(crate) remote_control: bool,

    /// Turn off Remote Control for this run.
    #[arg(long = "no-remote-control", overrides_with = "remote_control")]
    pub(crate) no_remote_control: bool,

    /// Run Claude as a series of `claude -p` calls instead of a terminal
    /// session. For machines with no terminal, such as CI.
    #[arg(long)]
    pub(crate) headless_exec: bool,

    /// With `--headless-exec`: the most `claude -p` calls for one issue before
    /// Ralphy marks it as stuck.
    #[arg(long, default_value_t = 6)]
    pub(crate) max_exec_calls: u32,

    /// When the agent reaches a usage limit, stop and show when the limit
    /// resets. Without this flag, Ralphy waits for the reset and then continues
    /// the same issue.
    // ADR-0003.
    #[arg(long)]
    pub(crate) stop_on_limit: bool,

    /// Send no Telegram messages for this run, even when Telegram is set up.
    // ADR-0007.
    #[arg(long)]
    pub(crate) no_telegram: bool,

    /// The title of the Telegram message for this run, instead of the automatic
    /// one.
    #[arg(long)]
    pub(crate) title: Option<String>,

    /// Do nothing (exit 0) when another run is already working in the repo.
    /// Scheduled runs use this, so a timer never starts a second run. Without
    /// it, Ralphy only warns.
    #[arg(long)]
    pub(crate) if_idle: bool,

    /// Work only the issues assigned to this GitHub login (`@me` is you).
    /// Overrides `queue.assignee` in the settings. `--only-issue` and
    /// `--issues` ignore it.
    #[arg(long)]
    pub(crate) assignee: Option<String>,

    /// Ignore the `queue.assignee` setting for this run. Cannot be used with
    /// `--assignee`.
    #[arg(long = "no-assignee", conflicts_with = "assignee")]
    pub(crate) no_assignee: bool,
}

#[derive(Args)]
pub(crate) struct ConsolidateArgs {
    /// Any folder inside the repo.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// The agent that merges the notes. The default is Claude. Every agent
    /// follows the same instructions.
    // ADR-0031.
    #[arg(long = "agent", value_enum, default_value_t = CliAgent::Claude)]
    pub(crate) agent: CliAgent,

    /// The model that merges the notes. Default: the agent's own default (opus
    /// for Claude).
    #[arg(long)]
    pub(crate) model: Option<String>,

    /// Reasoning effort for the merge (Claude and Codex only). Default for
    /// Claude: `medium`.
    #[arg(long)]
    pub(crate) effort: Option<String>,

    /// Stop the agent after this many minutes.
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
