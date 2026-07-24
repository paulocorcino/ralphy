use super::*;

#[test]
fn parse_commands_one_per_line() {
    let md = "# Plan\n\n## Verify\n\ncargo fmt --check\ncargo test -p ralphy-core\n\n## Next\n";
    let spec = parse_verify(md);
    assert_eq!(
        spec,
        VerifySpec::Commands(vec![
            vec!["cargo".into(), "fmt".into(), "--check".into()],
            vec![
                "cargo".into(),
                "test".into(),
                "-p".into(),
                "ralphy-core".into()
            ],
        ])
    );
}

#[test]
fn parse_none_is_opt_out() {
    assert_eq!(parse_verify("## Verify\n\nnone\n"), VerifySpec::None);
    // Case-insensitive.
    assert_eq!(parse_verify("## Verify\nNONE\n"), VerifySpec::None);
}

#[test]
fn parse_absent_section_is_unspecified() {
    assert_eq!(
        parse_verify("# Plan\n\n## Steps\n- [ ] do\n"),
        VerifySpec::Unspecified
    );
}

#[test]
fn parse_empty_section_is_unspecified() {
    let md = "## Verify\n\n## Notes\nstuff\n";
    assert_eq!(parse_verify(md), VerifySpec::Unspecified);
}

#[test]
fn parse_is_fence_tolerant() {
    let md = "## Verify\n\n```\ncargo test\n```\n";
    assert_eq!(
        parse_verify(md),
        VerifySpec::Commands(vec![vec!["cargo".into(), "test".into()]])
    );
}

#[test]
fn parse_quoted_args_stay_one_token() {
    let md = "## Verify\n\nsh -c \"cargo test --all\"\n";
    assert_eq!(
        parse_verify(md),
        VerifySpec::Commands(vec![vec![
            "sh".into(),
            "-c".into(),
            "cargo test --all".into()
        ]])
    );
}

#[test]
fn parse_single_quotes_too() {
    assert_eq!(
        tokenize("echo 'hello world' bye"),
        vec!["echo", "hello world", "bye"]
    );
}

/// A bulleted checklist is rejected at parse time: the leading `-` would tokenize
/// into a bogus `-` program the gate spawn-fails on, so the real command never runs
/// (#181). The error must name the offending line so the plan author can fix it.
#[test]
fn parse_bulleted_list_is_invalid() {
    let md = "## Verify\n\n- cargo test\n- cargo fmt --check\n";
    match parse_verify(md) {
        VerifySpec::Invalid(error) => {
            assert!(
                error.contains("one bare command per line"),
                "names the contract: {error}"
            );
            assert!(error.contains("`- cargo test`"), "names offender: {error}");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// A `- [ ]` checkbox line is markdown prose, not a command — rejected (#181).
#[test]
fn parse_checkbox_list_is_invalid() {
    let md = "## Verify\n\n- [ ] cargo test\n";
    match parse_verify(md) {
        VerifySpec::Invalid(error) => {
            assert!(
                error.contains("`- [ ] cargo test`"),
                "names offender: {error}"
            );
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// The real-world trigger (#181): a backtick-wrapped command with trailing prose.
/// The leading `-` and the backticks both mark it as markdown, not a command.
#[test]
fn parse_backtick_prose_is_invalid() {
    let md = "## Verify\n\n- `npm run lint` — passou, sem warnings\n";
    assert!(matches!(parse_verify(md), VerifySpec::Invalid(_)));
}

/// A `*`/`+` bullet or a bare backtick-wrapped command (no leading bullet) is also
/// markdown prose — the leading backtick alone marks it (#181).
#[test]
fn parse_star_bullet_and_bare_backtick_are_invalid() {
    assert!(matches!(
        parse_verify("## Verify\n\n* cargo test\n"),
        VerifySpec::Invalid(_)
    ));
    assert!(matches!(
        parse_verify("## Verify\n\n`cargo test`\n"),
        VerifySpec::Invalid(_)
    ));
}

/// The clean bare-command path is untouched: a section that is one bare command
/// per line still parses to `VerifySpec::Commands` (regression guard for #181).
#[test]
fn parse_bare_commands_unaffected() {
    let md = "## Verify\n\ncargo test\ncargo fmt --check\n";
    assert_eq!(
        parse_verify(md),
        VerifySpec::Commands(vec![
            vec!["cargo".into(), "test".into()],
            vec!["cargo".into(), "fmt".into(), "--check".into()],
        ])
    );
}

/// The live #268 trigger: a cursor plan authored a defensive `sh -c` check with a
/// nested backslash-escaped quote. The tokenizer cannot honor `\"`, so instead of
/// mis-splitting it into a garbage argv that fails opaquely at runtime (spending the
/// repair budget), it is rejected at parse time with an actionable message.
#[test]
fn parse_nested_escaped_quote_is_invalid() {
    let md = "## Verify\n\nsh -c \"test \\\"$(git diff-tree --name-only -r HEAD)\\\" = \\\"README.md\\\"\"\n";
    match parse_verify(md) {
        VerifySpec::Invalid(error) => {
            assert!(
                error.contains("no shell") && error.contains("backslash-escaped quotes"),
                "names the contract: {error}"
            );
            assert!(error.contains("sh -c"), "names offender: {error}");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// The two tokenizer-safe rewrites of the same check parse cleanly to `Commands`:
/// single outer quotes (double quotes stay literal inside `'...'`) and a bare
/// `python -c` one-liner (regression guard for #268 — no over-broad rejection).
#[test]
fn parse_escape_free_quotes_still_parse() {
    // Single outer quotes: the inner double quotes are literal, no backslash needed.
    let single = "## Verify\n\nsh -c 'test \"$x\" = y'\n";
    assert_eq!(
        parse_verify(single),
        VerifySpec::Commands(vec![vec![
            "sh".into(),
            "-c".into(),
            "test \"$x\" = y".into(),
        ]])
    );
    // A python one-liner with double-quoted arg — no escaped quotes, parses fine.
    let py = "## Verify\n\npython -c \"assert open('LAB.md').read()\"\n";
    assert!(matches!(parse_verify(py), VerifySpec::Commands(_)));
}

/// A Windows path in a verify command carries single backslashes before non-quote
/// chars — these must NOT be mistaken for escaped quotes (#268 false-positive guard).
#[test]
fn parse_windows_path_backslashes_are_not_escaped_quotes() {
    let md = "## Verify\n\npython C:\\tools\\check.py\n";
    assert_eq!(
        parse_verify(md),
        VerifySpec::Commands(vec![vec!["python".into(), "C:\\tools\\check.py".into(),]])
    );
}

#[test]
fn parse_stops_at_next_heading() {
    let md = "## Verify\ncargo test\n## Other\ncargo bogus\n";
    assert_eq!(
        parse_verify(md),
        VerifySpec::Commands(vec![vec!["cargo".into(), "test".into()]])
    );
}

#[test]
fn tail_keeps_whole_last_lines() {
    let s = "line1\nline2\nline3";
    assert_eq!(tail(s), "line1\nline2\nline3");
}

/// Regression: the byte cut at `len - TAIL_BYTES` can land inside a multi-byte
/// char (box-drawing '└' in vitest output). `tail` must nudge to a char boundary
/// instead of panicking. Each '└' is 3 bytes on 0/3/6… boundaries; TAIL_BYTES is
/// not a multiple of 3, so a pure run of '└' guarantees the cut lands mid-char.
#[test]
fn tail_does_not_split_multibyte_char() {
    let n = TAIL_BYTES; // plenty long so the cut is well inside the '└' run
    let s = format!("{}\nTAIL", "\u{2514}".repeat(n));
    let start = s.trim_end().len() - TAIL_BYTES;
    assert!(
        !s.is_char_boundary(start),
        "test setup: cut must be mid-char"
    );
    // Must not panic; the retained tail is whole-line and valid UTF-8.
    let out = tail(&s);
    assert_eq!(out, "TAIL");
    assert!(
        !out.contains('\u{FFFD}'),
        "no replacement chars from a bad split"
    );
}

/// A portable command that exits 0 on every platform: the OS shell builtin via
/// the host interpreter. We avoid a shell and instead use a tiny program both
/// platforms ship — but argv-only. On Windows `cmd /c exit 0`, elsewhere
/// `sh -c "exit 0"`.
fn ok_cmd() -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/c".into(), "exit 0".into()]
    } else {
        vec!["sh".into(), "-c".into(), "exit 0".into()]
    }
}

fn fail_cmd() -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/c".into(), "exit 3".into()]
    } else {
        vec!["sh".into(), "-c".into(), "exit 3".into()]
    }
}

#[test]
fn run_all_pass() {
    let dir = std::env::temp_dir();
    let report = run(&[ok_cmd(), ok_cmd()], &dir, Duration::from_secs(30));
    assert!(report.passed, "both ok commands pass");
    assert_eq!(report.commands.len(), 2);
    assert!(report.commands.iter().all(|c| c.passed()));
}

#[test]
fn run_stops_at_first_failure() {
    let dir = std::env::temp_dir();
    let report = run(
        &[ok_cmd(), fail_cmd(), ok_cmd()],
        &dir,
        Duration::from_secs(30),
    );
    assert!(!report.passed, "a non-zero exit fails the gate");
    // The third command never ran — the gate stops at the first failure.
    assert_eq!(report.commands.len(), 2, "stops after the failing command");
    assert_eq!(report.commands[1].exit_code, Some(3));
}

#[test]
fn run_spawn_failure_is_a_failure() {
    let dir = std::env::temp_dir();
    let report = run(
        &[vec!["definitely-not-a-real-binary-xyz".into()]],
        &dir,
        Duration::from_secs(30),
    );
    assert!(!report.passed, "an unspawnable command fails the gate");
    assert!(report.commands[0].output_tail.contains("failed to spawn"));
    // The command never ran: flagged spawn_failed distinctly from a signal kill
    // or a non-zero exit, and surfaced through the report (#182).
    assert!(
        report.commands[0].spawn_failed,
        "flagged as a spawn failure"
    );
    assert!(
        report.spawn_failed(),
        "the gate's failure is a spawn failure"
    );
}

/// An empty argv can't be run — treated as a spawn failure so the gate short-
/// circuits rather than handing back an unrepairable command (#182).
#[test]
fn run_empty_argv_is_a_spawn_failure() {
    let dir = std::env::temp_dir();
    let report = run(&[vec![]], &dir, Duration::from_secs(30));
    assert!(!report.passed);
    assert!(report.commands[0].spawn_failed);
    assert!(report.spawn_failed());
}

/// A real non-zero exit is NOT a spawn failure: the command ran and failed, so the
/// gate keeps its full repair budget (regression guard — the short-circuit must not
/// swallow a repairable test failure, #182).
#[test]
fn run_nonzero_exit_is_not_a_spawn_failure() {
    let dir = std::env::temp_dir();
    let report = run(&[fail_cmd()], &dir, Duration::from_secs(30));
    assert!(!report.passed);
    assert!(
        !report.commands[0].spawn_failed,
        "it ran, then exited non-zero"
    );
    assert!(
        !report.spawn_failed(),
        "a ran-and-failed gate is not a spawn failure"
    );
}

/// The gate's *deciding* failure is what counts: a passing command followed by an
/// unspawnable one is still a spawn failure (the short-circuit keys off the first
/// failure, which the gate stops at — #182).
#[test]
fn run_spawn_failure_after_a_pass_is_still_a_spawn_failure() {
    let dir = std::env::temp_dir();
    let report = run(
        &[ok_cmd(), vec!["definitely-not-a-real-binary-xyz".into()]],
        &dir,
        Duration::from_secs(30),
    );
    assert!(!report.passed);
    assert!(report.spawn_failed());
    assert_eq!(
        report.first_failure().map(|c| c.argv.as_slice()),
        Some(["definitely-not-a-real-binary-xyz".to_string()].as_slice())
    );
}

#[test]
fn comment_marks_pass_and_fail() {
    let report = VerifyReport {
        commands: vec![
            CommandOutcome {
                argv: vec!["cargo".into(), "fmt".into()],
                exit_code: Some(0),
                timed_out: false,
                spawn_failed: false,
                output_tail: String::new(),
                secs: 0.1,
            },
            CommandOutcome {
                argv: vec!["cargo".into(), "test".into()],
                exit_code: Some(101),
                timed_out: false,
                spawn_failed: false,
                output_tail: "panicked at assertion".into(),
                secs: 0.1,
            },
        ],
        passed: false,
    };
    let c = comment("stamp-1", &report);
    assert!(c.contains("## Verify (Ralphy run stamp-1)"));
    assert!(c.contains("\u{2713} cargo fmt"));
    assert!(c.contains("\u{2717} cargo test    exit 101"));
    assert!(c.contains("panicked at assertion"), "failing tail shown");
}

/// The honesty artifact for a malformed section (#181): same heading shape as the
/// gate-run comment, says plainly that nothing ran and the issue stayed open, and
/// carries the parse error naming the offender.
#[test]
fn invalid_comment_names_error_and_open_issue() {
    let error = "`## Verify` must be one bare command per line, not a markdown list. \
                 Offending line(s): `- cargo test`";
    let c = invalid_comment("stamp-7", error);
    assert!(c.contains("## Verify (Ralphy run stamp-7)"));
    assert!(c.contains("could NOT run"));
    assert!(c.contains("left open"));
    assert!(c.contains("`- cargo test`"), "carries the parse error");
}

/// The honesty artifact for a spawn failure (#182): same heading shape, names it a
/// spec/spawn problem (not a test failure), says no repair attempts were spent, and
/// shows both the "could not spawn" status line and the spawn error detail.
#[test]
fn spawn_failure_comment_names_spawn_problem_and_no_repairs() {
    let report = VerifyReport {
        commands: vec![CommandOutcome {
            argv: vec!["cargo".into(), "test".into()],
            exit_code: None,
            timed_out: false,
            spawn_failed: true,
            output_tail: "failed to spawn `cargo`: program not found".into(),
            secs: 0.0,
        }],
        passed: false,
    };
    let c = spawn_failure_comment("stamp-8", &report);
    assert!(c.contains("## Verify (Ralphy run stamp-8)"));
    assert!(c.contains("could NOT run"));
    assert!(
        c.contains("spec/spawn problem"),
        "named as spec/spawn, not test"
    );
    assert!(
        c.contains("WITHOUT spending any repair attempts"),
        "states no repair budget was burned"
    );
    assert!(
        c.contains("\u{2717} cargo test    could not spawn"),
        "status line names the unspawnable command"
    );
    assert!(c.contains("failed to spawn `cargo`"), "spawn error shown");
}

/// The shared status line renders a spawn failure distinctly from a signal kill:
/// "could not spawn" vs. "exit killed", both of which carry `exit_code: None`
/// (#182). Regression guard for the core distinction the issue is about.
#[test]
fn status_line_distinguishes_spawn_failure_from_kill() {
    let spawn = CommandOutcome {
        argv: vec!["nope".into()],
        exit_code: None,
        timed_out: false,
        spawn_failed: true,
        output_tail: String::new(),
        secs: 0.0,
    };
    let killed = CommandOutcome {
        argv: vec!["cargo".into(), "test".into()],
        exit_code: None,
        timed_out: false,
        spawn_failed: false,
        output_tail: String::new(),
        secs: 0.0,
    };
    assert_eq!(
        status_line(&spawn),
        "\u{2717} nope    could not spawn — program not found"
    );
    assert_eq!(status_line(&killed), "\u{2717} cargo test    exit killed");
}

/// The Windows batch-routing decision, isolated from PATH resolution: a resolved
/// `.cmd` routes through `cmd /C` (a batch file is not an executable image), a
/// resolved `.exe` runs directly, and an unresolved name passes through so the
/// spawn surfaces the same "program not found" failure. Resolution itself (the
/// PATHEXT/`.cmd`-shim search) is unit-tested in the `ralphy-proc-util` leaf crate.
#[cfg(windows)]
#[test]
fn spawn_argv_routes_cmd_shim_through_cmd_c() {
    use std::ffi::OsString;
    use std::path::PathBuf;

    assert_eq!(
        spawn_argv(
            Some(PathBuf::from("C:\\bin\\pnpm.cmd")),
            "pnpm",
            &["install".into()]
        ),
        (
            OsString::from("cmd"),
            vec![
                OsString::from("/C"),
                OsString::from("C:\\bin\\pnpm.cmd"),
                OsString::from("install"),
            ]
        )
    );
    assert_eq!(
        spawn_argv(
            Some(PathBuf::from("C:\\bin\\cargo.exe")),
            "cargo",
            &["test".into()]
        ),
        (
            OsString::from("C:\\bin\\cargo.exe"),
            vec![OsString::from("test")]
        )
    );
    assert_eq!(
        spawn_argv(None, "pnpm", &["install".into()]),
        (OsString::from("pnpm"), vec![OsString::from("install")])
    );
}

#[test]
fn repair_brief_names_failure_and_forbids_weakening() {
    let report = VerifyReport {
        commands: vec![CommandOutcome {
            argv: vec!["pnpm".into(), "install".into()],
            exit_code: Some(1),
            timed_out: false,
            spawn_failed: false,
            output_tail: "ERR_PNPM_LOCKFILE_MISMATCH".into(),
            secs: 0.1,
        }],
        passed: false,
    };
    let b = repair_brief("stamp-9", &report, "DONE_TOKEN");
    assert!(b.contains("repair required"));
    assert!(b.contains("\u{2717} pnpm install    exit 1"));
    assert!(
        b.contains("ERR_PNPM_LOCKFILE_MISMATCH"),
        "failing tail shown"
    );
    // The gate is the authority — the brief must forbid gaming it, quoting
    // the injected completion token rather than a hardcoded one.
    assert!(b.contains("`DONE_TOKEN`"));
    assert!(b.to_lowercase().contains("root cause"));
    assert!(
        b.contains("SAME"),
        "must say the runner re-runs the same commands"
    );
}
