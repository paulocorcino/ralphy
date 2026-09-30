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

/// Copilot's per-phase models and efforts ride the SHARED `run` flags
/// (`--plan-model`/`--exec-model`, `--plan-effort`/`--exec-effort`, merged at
/// `build_agent`) and `copilot.*` settings; `run` has no Copilot-only flag
/// (ADR-0044 D6).
#[test]
fn copilot_rides_the_shared_run_flags() {
    use clap::CommandFactory;
    let cli = Cli::command();
    let run = cli
        .get_subcommands()
        .find(|s| s.get_name() == "run")
        .expect("the `run` subcommand must be registered");
    let flags: Vec<&str> = run.get_arguments().filter_map(|a| a.get_long()).collect();
    for shared in ["plan-model", "exec-model", "plan-effort", "exec-effort"] {
        assert!(
            flags.contains(&shared),
            "`run --{shared}` is missing: {flags:?}"
        );
    }
    assert!(
        !flags.iter().any(|f| f.contains("copilot")),
        "`run` must have no Copilot-only flag: {flags:?}"
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
    assert!(matches!(
        args.command,
        Some(daemon::DaemonCommand::Setup {
            name: None,
            avatar: None
        })
    ));

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
fn daemon_install_help_names_the_real_mechanisms() {
    use clap::CommandFactory;

    let mut cmd = Cli::command();
    let install = cmd
        .find_subcommand_mut("daemon")
        .expect("the `daemon` subcommand")
        .find_subcommand_mut("install")
        .expect("the `daemon install` subcommand");
    let help = install.render_long_help().to_string();
    let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
    for mechanism in ["Run key", "systemd user unit", "launchd agent"] {
        assert!(
            help.contains(mechanism),
            "`daemon install --help` must name {mechanism:?}: {help}"
        );
    }
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
fn schedule_subcommands_parse() {
    use schedule::{ScheduleCommand as S, ScheduleTarget as T};
    type Check = fn(&S) -> bool;
    // (argv after `ralphy schedule`, what it must parse to)
    let rows: [(&[&str], Check); 4] = [
        (
            &["install", "run", "--every", "30m"],
            |c| matches!(c, S::Install { target: T::Run, every, .. } if every == "30m"),
        ),
        (&["install", "run", "--with-triage"], |c| {
            matches!(
                c,
                S::Install {
                    target: T::Run,
                    with_triage: true,
                    ..
                }
            )
        }),
        (
            &["install", "triage", "--every", "8h"],
            |c| matches!(c, S::Install { target: T::Triage, every, .. } if every == "8h"),
        ),
        (&["remove", "--all"], |c| {
            matches!(
                c,
                S::Remove {
                    target: None,
                    all: true,
                    ..
                }
            )
        }),
    ];
    for (args, check) in rows {
        let argv: Vec<&str> = ["ralphy", "schedule"].iter().chain(args).copied().collect();
        let cli = Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
        let Command::Schedule(command) = cli.command else {
            panic!("{argv:?}: expected the `schedule` subcommand");
        };
        assert!(check(&command), "{argv:?} parsed to the wrong command");
    }
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
/// `--agent <name>` parses every agent from its one-word name, and `cli_name`
/// round-trips it. The selector is the vendor name, even where the binary is
/// `cursor-agent`/`agent` (ADR-0042 D14), and `opencode` is one word (ADR-0005
/// D1) — clap's derived kebab spelling `open-code` stays accepted as an alias.
#[test]
fn every_cli_agent_parses_from_its_one_word_name() {
    use clap::ValueEnum;
    for agent in CliAgent::value_variants() {
        let name = agent.cli_name();
        assert_eq!(name, format!("{agent:?}").to_lowercase(), "{agent:?}");
        assert_eq!(CliAgent::from_str(name, false).ok(), Some(*agent), "{name}");
    }
    assert_eq!(
        CliAgent::from_str("open-code", false).ok(),
        Some(CliAgent::OpenCode),
        "the kebab alias"
    );
}
#[test]
fn run_help_lists_all_flags() {
    // Render the `run` subcommand's help and arg set and assert the flags that a
    // dropped clap attribute would lose are all present.
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
}

/// `--help` is text a user reads, and an ADR number or a `docs/adr` path means
/// something only to a developer of Ralphy. The decision stays in a `//`
/// comment next to the field; clap never prints those.
#[test]
fn no_help_text_cites_an_adr() {
    use clap::CommandFactory;

    fn walk(cmd: &mut clap::Command, path: &str, found: &mut Vec<String>) {
        let help = cmd.render_long_help().to_string();
        let cites = regex::Regex::new(r"ADR-\d|docs/adr").expect("a valid regex literal");
        found.extend(
            help.lines()
                .filter(|l| cites.is_match(l))
                .map(|l| format!("{path}: {}", l.trim())),
        );
        for sub in cmd.get_subcommands_mut() {
            let path = format!("{path} {}", sub.get_name());
            walk(sub, &path, found);
        }
    }

    let mut cmd = Cli::command();
    cmd.build();
    let mut found = Vec::new();
    walk(&mut cmd, "ralphy", &mut found);
    assert!(
        found.is_empty(),
        "--help text that cites an ADR:\n{}",
        found.join("\n")
    );
}

/// The commands the workbench and the agents run are hidden from the main list
/// and listed apart; hidden, they still parse, because the daemon spawns them.
#[test]
fn internal_commands_are_listed_apart_and_still_parse() {
    let mut cmd = command();
    let help = cmd.render_help().to_string();
    let (main, footer) = help
        .split_once("Run for you by the workbench and the agents:")
        .expect("the help has the heading of the internal commands");
    let names = |text: &str| -> Vec<String> {
        text.lines()
            .filter_map(|l| l.strip_prefix("  "))
            .filter_map(|l| l.split_whitespace().next())
            .map(str::to_string)
            .collect()
    };
    let internal = ["hook", "branch", "label", "changes", "blob", "sync"];
    assert_eq!(names(footer), internal);
    for name in internal {
        assert!(
            !names(main).contains(&name.to_string()),
            "{name} is in the main list"
        );
    }
    for visible in ["run", "daemon", "worktree", "update"] {
        assert!(
            names(main).contains(&visible.to_string()),
            "{visible} is missing"
        );
    }

    // Shapes typed by hand (the daemon's own argv are checked by
    // `every_argv_the_daemon_spawns_parses`). A dash-led worktree name after
    // `--` stays positional (core then refuses or misses it).
    for argv in [
        vec!["ralphy", "branch", "list"],
        vec!["ralphy", "branch", "list", "--format", "json"],
        vec!["ralphy", "branch", "switch", "feat"],
        vec!["ralphy", "branch", "create", "feat"],
        vec!["ralphy", "label", "set", "7", "--add", "x"],
        vec!["ralphy", "changes", "list"],
        vec!["ralphy", "changes", "list", "--format", "json"],
        vec![
            "ralphy",
            "changes",
            "discard",
            "--repo",
            ".",
            "--path=a.txt",
        ],
        vec![
            "ralphy",
            "blob",
            "read",
            "--revision",
            "head",
            "--path",
            "a",
        ],
        vec![
            "ralphy",
            "blob",
            "read",
            "--revision",
            "head",
            "--path",
            "a/b.rs",
            "--format",
            "json",
        ],
        vec!["ralphy", "worktree", "list", "--format", "json"],
        vec!["ralphy", "worktree", "add", "--base=main", "--", "wt-x"],
        vec!["ralphy", "worktree", "add", "--", "-x"],
        vec!["ralphy", "worktree", "remove", "--", "wt-x"],
        vec!["ralphy", "worktree", "remove", "--", "-x"],
        vec!["ralphy", "sync", "status"],
        vec!["ralphy", "hook", "status"],
        vec!["ralphy", "issues", "--format", "json", "--board"],
    ] {
        if let Err(e) = Cli::try_parse_from(&argv) {
            panic!("{argv:?} must still parse: {e}");
        }
    }
}

/// Every argv the daemon builds for a workbench button parses here. The argv
/// come from the daemon's own builders, so a flag renamed on either side fails.
#[test]
fn every_argv_the_daemon_spawns_parses() {
    use clap::ValueEnum;
    use ralphy_daemon::dispatch::{self as d, Verb};
    use serde_json::json;

    let mut built: Vec<Vec<String>> = vec![
        d::board_argv(),
        d::branch_list_argv(),
        d::worktree_list_argv(),
        d::changes_list_argv(),
        d::sync_status_argv(),
    ];
    let mut ok = |label: &str, argv: Result<Vec<String>, d::ArgvError>| {
        built.push(argv.unwrap_or_else(|e| panic!("{label}: the daemon refused: {e}")));
    };
    // Each agent name the CLI accepts is one the daemon launches.
    for agent in CliAgent::value_variants() {
        let name = agent.cli_name();
        ok(
            name,
            d::spawn_argv(
                Verb::Run,
                &json!({"agent": name, "planAgent": name, "branchMode": "new"}),
            ),
        );
    }
    ok(
        "run current",
        d::spawn_argv(
            Verb::Run,
            &json!({"agent": "claude", "branchMode": "current"}),
        ),
    );
    ok("triage", d::spawn_argv(Verb::Triage, &json!({})));
    ok("push", d::spawn_argv(Verb::PushQueue, &json!({})));
    ok("issue show", d::issue_show_argv(&json!({"number": 7})));
    ok(
        "blob read",
        d::blob_read_argv(&json!({"revision": "head", "path": "a/b.rs"})),
    );
    ok("run stop", d::run_stop_argv(&json!({"runid": "abc123"})));
    ok(
        "project remove",
        d::project_remove_argv(&json!({"slug": "owner/repo"})),
    );
    for verb in [Verb::SyncFetch, Verb::SyncPull, Verb::SyncPush] {
        ok("sync", d::sync_argv(verb));
    }
    for verb in [
        Verb::ChangesStage,
        Verb::ChangesUnstage,
        Verb::ChangesDiscard,
    ] {
        ok(
            "changes paths",
            d::changes_paths_argv(verb, &json!({"paths": ["a.txt", "b/c.rs"]})),
        );
    }
    ok(
        "changes commit",
        d::changes_commit_argv(&json!({"message": "-m fix"})),
    );
    for verb in [Verb::BranchSwitch, Verb::BranchCreate] {
        ok("branch", d::branch_argv(verb, &json!({"name": "feat/x"})));
    }
    ok(
        "worktree add",
        d::worktree_add_argv(&json!({"name": "wt-x", "base": "main"})),
    );
    ok(
        "worktree add",
        d::worktree_add_argv(&json!({"name": "wt-x"})),
    );
    ok(
        "worktree remove",
        d::worktree_remove_argv(&json!({"name": "-x"})),
    );
    for op in ["add", "remove"] {
        ok(
            "label",
            d::label_argv(&json!({"number": 7, "label": "x", "op": op})),
        );
    }
    ok("config get", d::config_argv(Verb::ConfigGet, &json!({})));
    ok(
        "config set",
        d::config_argv(
            Verb::ConfigSet,
            &json!({"key": "queue.label", "value": "-v"}),
        ),
    );
    ok(
        "config unset",
        d::config_argv(Verb::ConfigUnset, &json!({"key": "queue.label"})),
    );

    for argv in built {
        let full: Vec<&str> = std::iter::once("ralphy")
            .chain(argv.iter().map(String::as_str))
            .collect();
        if let Err(e) = Cli::try_parse_from(&full) {
            panic!("{full:?} is built by the daemon and must parse: {e}");
        }
    }
}
