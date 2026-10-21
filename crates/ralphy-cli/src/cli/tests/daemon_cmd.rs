use super::*;

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
    let Some(daemon::DaemonCommand::Add { path, init, create }) = args.command else {
        panic!("expected `daemon add`");
    };
    assert_eq!(path, std::path::PathBuf::from("."));
    assert!(init);
    assert!(!create, "a folder is created only when asked");

    let cli = Cli::try_parse_from(["ralphy", "daemon", "add", "."]).expect("daemon add must parse");
    let Command::Daemon(args) = cli.command else {
        panic!("expected the `daemon` subcommand");
    };
    let Some(daemon::DaemonCommand::Add { init, .. }) = args.command else {
        panic!("expected `daemon add`");
    };
    assert!(!init, "the flag must default off");

    // A new folder always gets a repository: `--create` needs `--init`.
    assert!(Cli::try_parse_from(["ralphy", "daemon", "add", "--create", "x"]).is_err());
    assert!(Cli::try_parse_from(["ralphy", "daemon", "add", "--init", "--create", "x"]).is_ok());
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
        d::gemini_root_argv(),
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
