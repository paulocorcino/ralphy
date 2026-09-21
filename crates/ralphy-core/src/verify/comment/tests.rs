use super::*;

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
