//! The clap CLI surface: the top-level `Cli`/`Command`, the `run`/`consolidate`
//! argument structs, the internal `Hook` subcommands, and the CLI-only
//! agent/branch-mode selector enums. Kept as a CLI concern separate from the
//! composition root (`main.rs`) and the run orchestrator (`run.rs`); the enums map
//! into the core's own types at the boundary (docs/adr/0004, docs/adr/0002).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use ralphy_core::BranchMode;

use crate::{
    config, daemon, init, install, issues, models, mutate, schedule, telegram, triage, usage,
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
    /// Run-lock-aware label mutation (ADR-0036 §6).
    #[command(subcommand)]
    Label(mutate::LabelCommand),
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
}

#[derive(Args)]
pub(crate) struct RunArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// Which agent CLI executes the run: `claude` (the default, a live PTY
    /// session), `codex` (headless `codex exec`), or `opencode`. Selects the
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

    /// Planning model (default: opus, or `claude.plan_model` in settings.json).
    #[arg(long)]
    pub(crate) plan_model: Option<String>,

    /// Planning effort (default: medium, or `claude.plan_effort` in
    /// settings.json).
    #[arg(long)]
    pub(crate) plan_effort: Option<String>,

    /// Force the execution model for the issue (overrides the plan's judgment).
    #[arg(long)]
    pub(crate) exec_model: Option<String>,

    /// OpenCode `--variant` (effort) passed through to `opencode run`. Omitted
    /// when unset so the adapter never sends a value the provider rejects
    /// (docs/adr/0005 D3). Only used by `--agent opencode`.
    #[arg(long)]
    pub(crate) exec_variant: Option<String>,

    /// Execution effort (default: medium, or `claude.exec_effort` in
    /// settings.json).
    #[arg(long)]
    pub(crate) exec_effort: Option<String>,

    /// Execution model used when the plan emits no complexity judgment
    /// (default: sonnet, or `claude.default_exec_model` in settings.json).
    #[arg(long)]
    pub(crate) default_exec_model: Option<String>,

    /// Per-issue wall-clock budget (minutes) before the session is reclaimed
    /// (default: ≈60 min, or `claude.max_minutes_per_issue` in settings.json).
    /// Pass `0` explicitly to disable the cap — the issue is then bounded only
    /// by `--deadline-hours`.
    #[arg(long)]
    pub(crate) max_minutes_per_issue: Option<u64>,

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
mod tests {
    use super::*;

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
    fn schedule_subcommand_is_registered() {
        use clap::CommandFactory;
        assert!(
            Cli::command()
                .get_subcommands()
                .any(|s| s.get_name() == "schedule"),
            "the `schedule` subcommand must be registered in the CLI"
        );
    }

    #[test]
    fn daemon_subcommand_parses_with_default_and_explicit_port() {
        let cli = Cli::try_parse_from(["ralphy", "daemon"]).expect("bare daemon must parse");
        let Command::Daemon(args) = cli.command else {
            panic!("expected the `daemon` subcommand");
        };
        assert_eq!(args.port, ralphy_daemon::DEFAULT_PORT);

        let cli = Cli::try_parse_from(["ralphy", "daemon", "--port", "9000"])
            .expect("daemon --port must parse");
        let Command::Daemon(args) = cli.command else {
            panic!("expected the `daemon` subcommand");
        };
        assert_eq!(args.port, 9000);
    }

    #[test]
    fn daemon_setup_and_status_subcommands_parse() {
        let cli =
            Cli::try_parse_from(["ralphy", "daemon", "setup"]).expect("daemon setup must parse");
        let Command::Daemon(args) = cli.command else {
            panic!("expected the `daemon` subcommand");
        };
        assert!(matches!(args.command, Some(daemon::DaemonCommand::Setup)));

        let cli =
            Cli::try_parse_from(["ralphy", "daemon", "status"]).expect("daemon status must parse");
        let Command::Daemon(args) = cli.command else {
            panic!("expected the `daemon` subcommand");
        };
        assert!(matches!(args.command, Some(daemon::DaemonCommand::Status)));
    }

    #[test]
    fn daemon_install_and_uninstall_subcommands_parse() {
        let cli = Cli::try_parse_from(["ralphy", "daemon", "install"])
            .expect("daemon install must parse");
        let Command::Daemon(args) = cli.command else {
            panic!("expected the `daemon` subcommand");
        };
        assert!(matches!(args.command, Some(daemon::DaemonCommand::Install)));

        let cli = Cli::try_parse_from(["ralphy", "daemon", "uninstall"])
            .expect("daemon uninstall must parse");
        let Command::Daemon(args) = cli.command else {
            panic!("expected the `daemon` subcommand");
        };
        assert!(matches!(
            args.command,
            Some(daemon::DaemonCommand::Uninstall)
        ));
    }

    #[test]
    fn daemon_bind_defaults_to_loopback() {
        let cli = Cli::try_parse_from(["ralphy", "daemon"]).expect("bare daemon must parse");
        let Command::Daemon(args) = cli.command else {
            panic!("expected the `daemon` subcommand");
        };
        assert_eq!(
            args.bind,
            "127.0.0.1".parse::<std::net::IpAddr>().unwrap(),
            "the default bind is loopback"
        );

        let cli = Cli::try_parse_from(["ralphy", "daemon", "--bind", "100.64.0.1"])
            .expect("daemon --bind must parse");
        let Command::Daemon(args) = cli.command else {
            panic!("expected the `daemon` subcommand");
        };
        assert_eq!(args.bind, "100.64.0.1".parse::<std::net::IpAddr>().unwrap());
    }

    #[test]
    fn schedule_install_run_parses() {
        let cli = Cli::try_parse_from(["ralphy", "schedule", "install", "run", "--every", "30m"])
            .expect("schedule install run must parse");
        let Command::Schedule(schedule::ScheduleCommand::Install { target, every, .. }) =
            cli.command
        else {
            panic!("expected the `schedule install` subcommand");
        };
        assert_eq!(every, "30m");
        assert!(matches!(target, schedule::ScheduleTarget::Run));
    }

    #[test]
    fn schedule_install_run_with_triage_parses() {
        let cli = Cli::try_parse_from(["ralphy", "schedule", "install", "run", "--with-triage"])
            .expect("schedule install run --with-triage must parse");
        let Command::Schedule(schedule::ScheduleCommand::Install {
            target,
            with_triage,
            ..
        }) = cli.command
        else {
            panic!("expected the `schedule install` subcommand");
        };
        assert!(with_triage);
        assert!(matches!(target, schedule::ScheduleTarget::Run));
    }

    #[test]
    fn schedule_install_triage_parses() {
        let cli = Cli::try_parse_from(["ralphy", "schedule", "install", "triage", "--every", "8h"])
            .expect("schedule install triage must parse");
        let Command::Schedule(schedule::ScheduleCommand::Install { target, every, .. }) =
            cli.command
        else {
            panic!("expected the `schedule install` subcommand");
        };
        assert_eq!(every, "8h");
        assert!(matches!(target, schedule::ScheduleTarget::Triage));
    }

    #[test]
    fn schedule_remove_all_parses() {
        let cli = Cli::try_parse_from(["ralphy", "schedule", "remove", "--all"])
            .expect("schedule remove --all must parse");
        let Command::Schedule(schedule::ScheduleCommand::Remove { target, all, .. }) = cli.command
        else {
            panic!("expected the `schedule remove` subcommand");
        };
        assert!(all);
        assert!(target.is_none());
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
    fn cli_agent_parses_kimi() {
        // `--agent kimi` parses to the one-word variant and round-trips its cli_name.
        use clap::ValueEnum;
        assert_eq!(CliAgent::from_str("kimi", true).ok(), Some(CliAgent::Kimi));
        assert_eq!(CliAgent::Kimi.cli_name(), "kimi");
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
    fn branch_switch_and_create_subcommands_parse() {
        let cli = Cli::try_parse_from(["ralphy", "branch", "switch", "feat"])
            .expect("branch switch must parse");
        let Command::Branch(mutate::BranchCommand::Switch(a)) = cli.command else {
            panic!("expected `branch switch`");
        };
        assert_eq!(a.name, "feat");

        let cli = Cli::try_parse_from(["ralphy", "branch", "create", "feat"])
            .expect("branch create must parse");
        let Command::Branch(mutate::BranchCommand::Create(a)) = cli.command else {
            panic!("expected `branch create`");
        };
        assert_eq!(a.name, "feat");
    }

    #[test]
    fn label_set_subcommand_parses() {
        let cli = Cli::try_parse_from(["ralphy", "label", "set", "7", "--add", "AFK"])
            .expect("label set must parse");
        let Command::Label(mutate::LabelCommand::Set(a)) = cli.command else {
            panic!("expected `label set`");
        };
        assert_eq!(a.issue, 7);
        assert_eq!(a.add, vec!["AFK".to_string()]);
    }

    #[test]
    fn run_help_lists_all_flags() {
        // Guard the CLI-def move: render the `run` subcommand's help and arg set and
        // assert the flags that a botched attribute-drop would silently lose are all
        // present, plus that the `opencode` value keeps its `open-code` alias.
        use clap::CommandFactory;
        let cli = Cli::command();
        let run = cli
            .get_subcommands()
            .find(|s| s.get_name() == "run")
            .expect("the `run` subcommand must be registered");

        let long_ids: Vec<String> = run
            .get_arguments()
            .filter_map(|a| a.get_long().map(str::to_owned))
            .collect();
        for flag in ["plan-agent", "no-assignee", "if-idle"] {
            assert!(
                long_ids.iter().any(|l| l == flag),
                "run --help must list --{flag}; got {long_ids:?}"
            );
        }

        // The rendered long help must also carry the flags verbatim.
        let help = run.clone().render_long_help().to_string();
        for flag in ["--plan-agent", "--no-assignee", "--if-idle"] {
            assert!(help.contains(flag), "run --help text must mention {flag}");
        }

        // The `opencode` agent value resolves under both its canonical spelling and
        // the derived `open-code` alias.
        use clap::ValueEnum;
        assert_eq!(
            CliAgent::from_str("opencode", false).ok(),
            Some(CliAgent::OpenCode)
        );
        assert_eq!(
            CliAgent::from_str("open-code", false).ok(),
            Some(CliAgent::OpenCode)
        );
    }
}
