use super::*;

const NO_SUCH_TOOL: &str = "ralphy-gate-test-no-such-tool";

fn cargo_step(name: &'static str, args: &'static [&'static str]) -> Step {
    Step {
        name,
        ci: &[],
        program: "cargo",
        args,
        env: &[],
        needs: None,
        slow: false,
    }
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_only_flag_is_fast() {
    assert_eq!(parse_args(&[]).unwrap(), Options { fast: false });
    assert_eq!(
        parse_args(&args(&["--fast"])).unwrap(),
        Options { fast: true }
    );
    assert!(parse_args(&args(&["--only"])).is_err());
}

#[test]
fn a_failed_step_does_not_stop_the_gate() {
    let root = tempfile::tempdir().unwrap();
    let steps = [
        cargo_step("bad", &["--no-such-flag-for-the-gate-test"]),
        cargo_step("good", &["--version"]),
    ];
    let outcomes = run_all(
        root.path(),
        &steps,
        &Options { fast: false },
        Path::new("unused"),
    )
    .unwrap();
    assert!(matches!(outcomes[0], Outcome::Failed(Some(code)) if code != 0));
    assert_eq!(outcomes[1], Outcome::Passed);
}

#[test]
fn a_program_that_is_not_installed_is_missing() {
    let root = tempfile::tempdir().unwrap();
    let step = Step {
        program: NO_SUCH_TOOL,
        ..cargo_step("absent", &[])
    };
    let log = root.path().join("absent.log");
    assert_eq!(
        run_step(root.path(), &step, Path::new("unused"), &log).unwrap(),
        Outcome::Missing
    );
    assert!(Outcome::Missing.is_bad(), "a missing tool is never a pass");
}

#[test]
fn a_cargo_subcommand_that_is_not_installed_is_missing() {
    let root = tempfile::tempdir().unwrap();
    let step = Step {
        needs: Some(NO_SUCH_TOOL),
        ..cargo_step("sub", &["--version"])
    };
    let log = root.path().join("sub.log");
    assert_eq!(
        run_step(root.path(), &step, Path::new("unused"), &log).unwrap(),
        Outcome::Missing
    );
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        text.contains(&format!("{NO_SUCH_TOOL} is not on PATH")),
        "{text}"
    );
}

#[test]
fn fast_skips_only_the_slow_steps() {
    let root = tempfile::tempdir().unwrap();
    let steps = [
        Step {
            program: NO_SUCH_TOOL,
            slow: true,
            ..cargo_step("slow", &[])
        },
        cargo_step("quick", &["--version"]),
    ];
    let fast = run_all(
        root.path(),
        &steps,
        &Options { fast: true },
        Path::new("unused"),
    )
    .unwrap();
    assert_eq!(fast, [Outcome::Skipped, Outcome::Passed]);
    assert!(!Outcome::Skipped.is_bad());
    let full = run_all(
        root.path(),
        &steps,
        &Options { fast: false },
        Path::new("unused"),
    )
    .unwrap();
    assert_eq!(full[0], Outcome::Missing);
}

#[test]
fn the_log_holds_the_command_and_its_output() {
    let root = tempfile::tempdir().unwrap();
    let step = Step {
        env: &[("RALPHY_GATE_TEST", "1")],
        ci: &["Format check"],
        ..cargo_step("version", &["--version"])
    };
    run_all(
        root.path(),
        &[step],
        &Options { fast: false },
        Path::new("unused"),
    )
    .unwrap();
    let text = std::fs::read_to_string(log_dir(root.path()).join("version.log")).unwrap();
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some("$ RALPHY_GATE_TEST=\"1\" cargo --version")
    );
    assert_eq!(
        lines.next(),
        Some("# the local form of CI step: Format check")
    );
    assert!(
        lines.next().is_some_and(|l| l.starts_with("cargo ")),
        "{text}"
    );
}

#[test]
fn every_step_has_its_own_log_name() {
    let mut names: Vec<&str> = STEPS.iter().map(|s| s.name).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(names.len(), count, "two steps would write one log");
}

#[test]
fn an_xtask_step_runs_the_program_of_the_gate() {
    let root = tempfile::tempdir().unwrap();
    let step = Step {
        program: steps::XTASK,
        ..cargo_step("self", &["--version"])
    };
    let log = root.path().join("self.log");
    // `cargo` stands in for the gate's own program.
    let outcome = run_step(root.path(), &step, Path::new("cargo"), &log).unwrap();
    assert_eq!(outcome, Outcome::Passed);
}

#[test]
fn park_moves_the_program_over_an_old_copy() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("xtask.exe");
    let parked = dir.path().join("xtask-running.exe");
    std::fs::write(&exe, "new").unwrap();
    std::fs::write(&parked, "old").unwrap();
    park(&exe, &parked).unwrap();
    assert!(!exe.exists(), "cargo may write a new program in its place");
    assert_eq!(std::fs::read_to_string(&parked).unwrap(), "new");
}
