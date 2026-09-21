use super::*;

/// The contract test for #302: the daemon's detail verb composes exactly the
/// ADR-0020 documented form, so the CLI must parse it. Deliberately
/// parse-level — no tracker, no network, no authentication.
#[test]
fn issues_show_documented_form_parses() {
    let parsed = Cli::try_parse_from(["ralphy", "issues", "show", "302", "--format", "json"]);
    let cli = match parsed {
        Ok(cli) => cli,
        Err(e) => panic!("the ADR-0020 documented form must parse: {e}"),
    };
    let Command::Issues(args) = cli.command else {
        panic!("expected the issues command");
    };
    // Not just "it parsed": a permissive positional that swallowed the tokens
    // would also be Ok, and the detail would still never load.
    assert_eq!(args.spec, ["show", "302"]);
}

#[test]
fn run_effort_flags_accept_only_the_core_lexicon() {
    let cli = Cli::try_parse_from([
        "ralphy",
        "run",
        "--plan-effort",
        "high",
        "--exec-effort",
        "max",
    ])
    .expect("canonical effort flags must parse");
    let Command::Run(args) = cli.command else {
        panic!("expected run command");
    };
    assert_eq!(args.plan_effort, Some(Effort::High));
    assert_eq!(args.exec_effort, Some(Effort::Max));

    for invalid in ["none", "minimal", "hihg"] {
        assert!(
            Cli::try_parse_from(["ralphy", "run", "--plan-effort", invalid]).is_err(),
            "accepted invalid effort {invalid}"
        );
    }
}

/// This slice (#232) wires Copilot's per-phase models through the EXISTING
/// `--plan-model`/`--exec-model` flags and `copilot.*` settings — no new
/// `run` flag. Pins the flag count captured on HEAD before the change.
#[test]
fn no_new_run_flags_for_copilot_model() {
    use clap::CommandFactory;
    let cli = Cli::command();
    let run = cli
        .get_subcommands()
        .find(|s| s.get_name() == "run")
        .expect("the `run` subcommand must be registered");
    let n = run
        .get_arguments()
        .filter(|a| a.get_long().is_some())
        .count();
    assert_eq!(n, 29, "this slice must introduce no new run flag");
}

/// Effort reaches Copilot via the existing `--plan-effort`/`--exec-effort`
/// flags (merged at `build_agent`); the clamp still introduces no new run
/// flag — `copilot.*_effort` remain settings.json keys for seven-rung
/// extensions (ADR-0044 D6).
#[test]
fn no_new_run_flags_for_copilot_effort() {
    use clap::CommandFactory;
    let cli = Cli::command();
    let run = cli
        .get_subcommands()
        .find(|s| s.get_name() == "run")
        .expect("the `run` subcommand must be registered");
    let n = run
        .get_arguments()
        .filter(|a| a.get_long().is_some())
        .count();
    assert_eq!(n, 29, "the effort clamp must introduce no new run flag");
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
fn update_subcommand_is_registered_and_defaults_to_the_rc_channel() {
    use clap::CommandFactory;
    assert!(
        Cli::command()
            .get_subcommands()
            .any(|s| s.get_name() == "update"),
        "the `update` subcommand must be registered in the CLI"
    );

    let cli = Cli::try_parse_from(["ralphy", "update"]).expect("bare update must parse");
    let Command::Update(args) = cli.command else {
        panic!("expected the update subcommand");
    };
    // The default is the channel the project actually ships on; a stable
    // default would make `update` blind for as long as it ships candidates.
    assert_eq!(args.channel, "rc");
    assert!(!args.check, "a bare update is not a dry run");

    let cli = Cli::try_parse_from(["ralphy", "update", "--check", "--channel", "stable"])
        .expect("update --check --channel stable must parse");
    let Command::Update(args) = cli.command else {
        panic!("expected the update subcommand");
    };
    assert!(args.check);
    assert_eq!(args.channel, "stable");
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

/// `--init` is the non-interactive path for `daemon add` (#363): it must be
/// off unless spelled, since its absence is what keeps a piped caller from
/// silently getting a repository it never asked for.
#[test]
fn daemon_add_init_flag_parses() {
    let cli = Cli::try_parse_from(["ralphy", "daemon", "add", "--init", "."])
        .expect("daemon add --init must parse");
    let Command::Daemon(args) = cli.command else {
        panic!("expected the `daemon` subcommand");
    };
    let Some(daemon::DaemonCommand::Add { path, init }) = args.command else {
        panic!("expected `daemon add`");
    };
    assert_eq!(path, std::path::PathBuf::from("."));
    assert!(init);

    let cli = Cli::try_parse_from(["ralphy", "daemon", "add", "."]).expect("daemon add must parse");
    let Command::Daemon(args) = cli.command else {
        panic!("expected the `daemon` subcommand");
    };
    let Some(daemon::DaemonCommand::Add { init, .. }) = args.command else {
        panic!("expected `daemon add`");
    };
    assert!(!init, "the flag must default off");
}

#[test]
fn daemon_setup_and_status_subcommands_parse() {
    let cli = Cli::try_parse_from(["ralphy", "daemon", "setup"]).expect("daemon setup must parse");
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
    let cli =
        Cli::try_parse_from(["ralphy", "daemon", "install"]).expect("daemon install must parse");
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

/// Reaching the daemon by NAME is an explicit declaration (docs/adr/0032 §4):
/// the cross-site gate refuses any `Host` it was not told about, which is what
/// keeps DNS rebinding out. Repeatable, and empty by default so a plain
/// loopback daemon declares nothing.
#[test]
fn daemon_allowed_hosts_are_declared_and_repeatable() {
    let cli = Cli::try_parse_from(["ralphy", "daemon"]).expect("bare daemon must parse");
    let Command::Daemon(args) = cli.command else {
        panic!("expected the `daemon` subcommand");
    };
    assert!(
        args.allowed_hosts.is_empty(),
        "the default declares no extra host"
    );

    let cli = Cli::try_parse_from([
        "ralphy",
        "daemon",
        "--bind",
        "100.64.0.1",
        "--allowed-host",
        "desk.tailnet.ts.net",
        "--allowed-host",
        "desk",
    ])
    .expect("daemon --allowed-host must parse");
    let Command::Daemon(args) = cli.command else {
        panic!("expected the `daemon` subcommand");
    };
    assert_eq!(args.allowed_hosts, ["desk.tailnet.ts.net", "desk"]);
}

#[test]
fn schedule_install_run_parses() {
    let cli = Cli::try_parse_from(["ralphy", "schedule", "install", "run", "--every", "30m"])
        .expect("schedule install run must parse");
    let Command::Schedule(schedule::ScheduleCommand::Install { target, every, .. }) = cli.command
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
    let Command::Schedule(schedule::ScheduleCommand::Install { target, every, .. }) = cli.command
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
    let cli =
        Cli::try_parse_from(["ralphy", "run", "--if-idle"]).expect("run with --if-idle must parse");
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
fn cli_agent_parses_copilot() {
    // `--agent copilot` parses to the one-word variant and round-trips its cli_name.
    use clap::ValueEnum;
    assert_eq!(
        CliAgent::from_str("copilot", true).ok(),
        Some(CliAgent::Copilot)
    );
    assert_eq!(CliAgent::Copilot.cli_name(), "copilot");
}

#[test]
fn cli_agent_parses_cursor() {
    // `--agent cursor` parses to the one-word variant and round-trips its
    // cli_name. The SELECTOR is the vendor name even though the binary is
    // `cursor-agent`/`agent` (ADR-0042 D14).
    use clap::ValueEnum;
    assert_eq!(
        CliAgent::from_str("cursor", true).ok(),
        Some(CliAgent::Cursor)
    );
    assert_eq!(CliAgent::Cursor.cli_name(), "cursor");
}

#[test]
fn cli_agent_parses_gemini() {
    // `--agent gemini` parses to the one-word variant and round-trips its
    // cli_name (ADR-0043 D1).
    use clap::ValueEnum;
    assert_eq!(
        CliAgent::from_str("gemini", true).ok(),
        Some(CliAgent::Gemini)
    );
    assert_eq!(CliAgent::Gemini.cli_name(), "gemini");
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

    let cli = Cli::try_parse_from(["ralphy", "branch", "list", "--format", "json"])
        .expect("branch list must parse");
    let Command::Branch(mutate::BranchCommand::List(a)) = cli.command else {
        panic!("expected `branch list`");
    };
    assert_eq!(a.format.as_deref(), Some("json"));
}

#[test]
fn worktree_list_subcommand_parses() {
    let cli = Cli::try_parse_from(["ralphy", "worktree", "list", "--format", "json"])
        .expect("worktree list must parse");
    let Command::Worktree(mutate::WorktreeCommand::List(a)) = cli.command else {
        panic!("expected `worktree list`");
    };
    assert_eq!(a.format.as_deref(), Some("json"));
    assert_eq!(a.repo, PathBuf::from("."));
}

#[test]
fn worktree_add_subcommand_parses() {
    let cli = Cli::try_parse_from(["ralphy", "worktree", "add", "--base=main", "--", "wt-x"])
        .expect("worktree add must parse");
    let Command::Worktree(mutate::WorktreeCommand::Add(a)) = cli.command else {
        panic!("expected `worktree add`");
    };
    assert_eq!(a.name, "wt-x");
    assert_eq!(a.base.as_deref(), Some("main"));
    assert_eq!(a.repo, PathBuf::from("."));

    // The `--` guard keeps a dash-led name positional; core refuses it.
    let cli = Cli::try_parse_from(["ralphy", "worktree", "add", "--", "-x"])
        .expect("worktree add -- -x must parse");
    let Command::Worktree(mutate::WorktreeCommand::Add(a)) = cli.command else {
        panic!("expected `worktree add`");
    };
    assert_eq!(a.name, "-x");
    assert!(a.base.is_none());
}

#[test]
fn worktree_remove_subcommand_parses() {
    let cli = Cli::try_parse_from(["ralphy", "worktree", "remove", "--", "wt-x"])
        .expect("worktree remove must parse");
    let Command::Worktree(mutate::WorktreeCommand::Remove(a)) = cli.command else {
        panic!("expected `worktree remove`");
    };
    assert_eq!(a.name, "wt-x");
    assert_eq!(a.repo, PathBuf::from("."));

    // The `--` guard keeps a dash-led name positional; core answers NotFound.
    let cli = Cli::try_parse_from(["ralphy", "worktree", "remove", "--", "-x"])
        .expect("worktree remove -- -x must parse");
    let Command::Worktree(mutate::WorktreeCommand::Remove(a)) = cli.command else {
        panic!("expected `worktree remove`");
    };
    assert_eq!(a.name, "-x");
}

#[test]
fn changes_list_subcommand_parses() {
    let cli = Cli::try_parse_from(["ralphy", "changes", "list", "--format", "json"])
        .expect("changes list must parse");
    let Command::Changes(changes::ChangesCommand::List(a)) = cli.command else {
        panic!("expected `changes list`");
    };
    assert_eq!(a.format.as_deref(), Some("json"));
}

#[test]
fn changes_discard_subcommand_parses() {
    let cli = Cli::try_parse_from([
        "ralphy",
        "changes",
        "discard",
        "--repo",
        ".",
        "--path=a.txt",
    ])
    .expect("changes discard must parse");
    let Command::Changes(changes::ChangesCommand::Discard(a)) = cli.command else {
        panic!("expected `changes discard`");
    };
    assert_eq!(a.path, vec!["a.txt".to_string()]);
}

#[test]
fn blob_read_subcommand_parses() {
    let cli = Cli::try_parse_from([
        "ralphy",
        "blob",
        "read",
        "--revision",
        "head",
        "--path",
        "a/b.rs",
        "--format",
        "json",
    ])
    .expect("blob read must parse");
    let Command::Blob(blob::BlobCommand::Read(a)) = cli.command else {
        panic!("expected `blob read`");
    };
    assert_eq!(a.path, "a/b.rs");
    assert_eq!(a.format.as_deref(), Some("json"));
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
