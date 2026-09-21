use super::*;

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
