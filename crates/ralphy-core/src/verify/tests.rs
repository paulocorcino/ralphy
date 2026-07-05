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

/// On Windows `pnpm` ships as a bare bash shim AND a `pnpm.cmd`; only the
/// `.cmd` is executable, and PATHEXT resolution must return it — not the
/// extensionless file a bare `CreateProcess` would choke on. This is the exact
/// failure that left a Node monorepo's gate red with "program not found".
#[cfg(windows)]
#[test]
fn resolve_in_finds_cmd_shim_not_the_bare_shell_file() {
    let dir = std::env::temp_dir().join(format!("ralphy-verify-resolve-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("pnpm"), "#!/bin/sh\n").unwrap(); // bash shim
    std::fs::write(dir.join("pnpm.cmd"), "@echo off\n").unwrap(); // cmd shim
    let exts = [".COM", ".EXE", ".BAT", ".CMD"];

    let got =
        resolve_in("pnpm", std::slice::from_ref(&dir), &exts).expect("pnpm resolves via PATHEXT");
    // Filesystem is case-insensitive; compare on name (lowercased) + parent so
    // a `.CMD`-vs-`.cmd` PathBuf mismatch doesn't fail a correct resolution.
    assert_eq!(
        got.file_name().unwrap().to_string_lossy().to_lowercase(),
        "pnpm.cmd",
        "resolves to the .cmd shim, not the bare bash file"
    );
    assert_eq!(got.parent().unwrap(), dir);

    assert!(
        resolve_in("definitely-absent-xyz", std::slice::from_ref(&dir), &exts).is_none(),
        "a missing program stays unresolved"
    );

    std::fs::remove_dir_all(&dir).ok();
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
