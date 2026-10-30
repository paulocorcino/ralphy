use super::*;

mod daemon_cmd;

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

    for invalid in ["none", "minimal", "extreme"] {
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
    let internal = [
        "hook", "branch", "label", "changes", "blob", "sync", "gemini",
    ];
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
