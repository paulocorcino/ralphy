use super::*;
use ralphy_adapter_support::PROMPT_EXECUTE;

const LOGGED_OUT: &str = "Not logged in \u{00b7} Please run /login\n";

/// A plan run that ended on its own, cleanly or not.
fn ended(exited_cleanly: bool) -> RunFacts {
    RunFacts {
        exited_cleanly,
        timed_out: false,
        stopped: false,
    }
}

#[test]
fn a_written_plan_with_auth_text_is_kept() {
    let dir = tempfile::tempdir().expect("tempdir");
    let plan_path = dir.path().join("plan.md");
    fs::write(&plan_path, "- [ ] one step\n").expect("write the plan");
    let plan = plan_from_run(
        &plan_path,
        &dir.path().join("plan.log"),
        ended(true),
        LOGGED_OUT,
    )
    .expect("a clean run that wrote a plan keeps it");
    assert_eq!(plan.path, plan_path);
    assert_eq!(plan.open_steps, 1);
}

#[test]
fn a_failed_plan_run_with_auth_text_is_an_auth_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let plan_path = dir.path().join("plan.md");
    let log_path = dir.path().join("plan.log");
    let want = format!("{CLAUDE_AUTH_ERROR_MSG} (see {})", log_path.display());
    // No plan written, clean exit.
    let err = plan_from_run(&plan_path, &log_path, ended(true), LOGGED_OUT)
        .expect_err("no plan and a login error is an auth failure");
    assert_eq!(err.to_string(), want);
    // A plan on disk, but the run failed.
    fs::write(&plan_path, "- [ ] one step\n").expect("write the plan");
    let err = plan_from_run(&plan_path, &log_path, ended(false), LOGGED_OUT)
        .expect_err("a failed run with a login error is an auth failure");
    assert_eq!(err.to_string(), want);
}

#[test]
fn planning_command_maps_high_and_omits_unset() {
    let set = planning_args(
        Some("opus"),
        Some("high"),
        Path::new("settings.json"),
        Path::new("plugin"),
    );
    assert_eq!(
        set.windows(2)
            .filter(|pair| pair == &["--effort", "high"])
            .count(),
        1
    );
    let unset = planning_args(None, None, Path::new("settings.json"), Path::new("plugin"));
    assert!(!unset.iter().any(|arg| arg == "--effort"));
}

/// Anti-drift: the charter this adapter launches sessions with and the
/// embedded execution prompt must both name the shared completion sentinel;
/// `ralphy_adapter_support::DONE_SENTINEL` is the single source of truth.
#[test]
fn charter_and_prompt_name_the_done_sentinel() {
    assert!(EXEC_CHARTER.contains(ralphy_adapter_support::DONE_SENTINEL));
    assert!(PROMPT_EXECUTE.contains(ralphy_adapter_support::DONE_SENTINEL));
}

#[test]
fn every_headless_argv_holds_its_prompt() {
    let (settings, plugin) = (Path::new("s.json"), Path::new("plugin"));
    let task = tasks::task_args(None, None, settings);
    // Consolidate, diagnose, draft and triage build one argv.
    let argvs = [
        ("plan", planning_args(None, None, settings, plugin)),
        (
            "execute -p",
            headless::headless_args(settings, plugin, "sonnet", None),
        ),
        ("consolidate", task.clone()),
        ("diagnose", task.clone()),
        ("draft", task.clone()),
        ("triage", task),
    ];
    for (name, argv) in argvs {
        for flags in [
            ["--input-format", "stream-json"],
            ["--output-format", "stream-json"],
        ] {
            assert!(argv.windows(2).any(|w| w == flags), "{name}: {argv:?}");
        }
    }
}

/// Every headless spawn holds its prompt behind the proof gate. A source
/// pin over the production text, because the spawns need a vendor binary.
#[test]
fn every_headless_spawn_holds_its_prompt() {
    for (name, src) in [
        ("lib.rs", include_str!("lib.rs")),
        ("headless.rs", include_str!("headless.rs")),
        ("tasks.rs", include_str!("tasks.rs")),
    ] {
        let code: String = src.split_whitespace().collect();
        let code = code.split("#[cfg(test)]mod").next().unwrap_or_default();
        for open in ["run_text_session(", "run_json_session(", "run_headless("] {
            assert!(!code.contains(open), "{name} calls {open} with no gate");
        }
        for call in code.split("HeadlessCall::new(").skip(1) {
            let builder = call.split(".run()").next().unwrap_or_default();
            assert!(
                builder.contains(".hold_prompt("),
                "{name}: a HeadlessCall with no gate"
            );
        }
    }
    let tasks: String = include_str!("tasks.rs").split_whitespace().collect();
    assert_eq!(
        tasks.matches("_session_held(").count(),
        4,
        "the four tasks hold their prompt"
    );
}

/// The per-issue minutes reach the exec config, and the run deadline clamps them.
#[test]
fn exec_budget_setters_reach_the_issue_deadline() {
    let run_deadline = Instant::now() + std::time::Duration::from_secs(1);
    let agent = ClaudeAgent::new(None, None, PathBuf::from("/run"))
        .with_exec_config(None, None, "sonnet".into(), 120, false, false, 3)
        .with_run_deadline(Some(run_deadline));
    assert_eq!(agent.exec.max_minutes_per_issue, 120);
    assert_eq!(agent.issue_deadline(), run_deadline);
}
