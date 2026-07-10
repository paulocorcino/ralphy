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
}

#[test]
fn comment_marks_pass_and_fail() {
    let report = VerifyReport {
        commands: vec![
            CommandOutcome {
                argv: vec!["cargo".into(), "fmt".into()],
                exit_code: Some(0),
                timed_out: false,
                output_tail: String::new(),
                secs: 0.1,
            },
            CommandOutcome {
                argv: vec!["cargo".into(), "test".into()],
                exit_code: Some(101),
                timed_out: false,
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
