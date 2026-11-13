//! The adapter end to end against `agy_test_child`, a fake `agy`, in temp git
//! repositories. The fake reads its scenario from `<root>/scenario` and
//! records its stdin lines and its `SessionStart` in `<root>/order.log`.
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ralphy_core::{Agent, HooksUnproven, Outcome, Plan, ResetAt};

mod common;

use common::{issue, read, Setup};

/// What the fake prints for `--version` (`VERSION` in the child).
const FAKE_VERSION: &str = "1.5.6-test";

/// The reset of the spent bucket in the fake's quota read-out (`RESET`).
const FAKE_RESET: &str = "2026-11-12T20:00:00Z";

impl Setup {
    fn run_log(&self) -> String {
        read(&self.run_dir.join("antigravity-1.log"))
    }
}

fn plan() -> Plan {
    Plan {
        path: PathBuf::from(".ralphy/plan.md"),
        open_steps: 1,
        recommended_model: None,
        usage: ralphy_core::Usage::default(),
        session_id: None,
    }
}

#[test]
fn execute_with_a_fake_agy_writes_the_task_only_after_session_start() {
    let setup = Setup::new("done");
    setup
        .agent(None)
        .execute(&plan(), &setup.ws())
        .expect("execute");
    let order = setup.order();
    assert_eq!(order.len(), 3, "{order:#?}");
    assert!(
        order[0].contains("Reply with the single word READY."),
        "the proof line comes first: {order:#?}"
    );
    assert_eq!(order[1], "SessionStart", "{order:#?}");
    assert!(order[2].starts_with("stdin: "), "{order:#?}");
}

#[test]
fn execute_with_a_fake_agy_returns_done_and_the_summed_usage() {
    let setup = Setup::new("done");
    let model = "gemini-3.8-flash-low";
    let execution = setup
        .agent(Some(model))
        .execute(&plan(), &setup.ws())
        .expect("execute");
    assert_eq!(execution.outcome, Outcome::Done);
    let usage = execution.usage;
    assert_eq!(
        (
            usage.input,
            usage.cache_read,
            usage.output,
            usage.cache_creation
        ),
        (300, 30, 12, 0),
        "the proof turn and the task turn, summed"
    );
    assert_eq!(usage.model.as_deref(), Some(model));
    assert_eq!(
        execution.session_id.as_deref(),
        Some("00000000-0000-4000-8000-0000000000aa")
    );
}

#[test]
fn a_missing_session_start_ends_execute_with_the_proof_error() {
    let setup = Setup::new("no_hook");
    let err = setup
        .agent(None)
        .execute(&plan(), &setup.ws())
        .expect_err("no proof, no task");
    assert!(err.chain().any(is_unproven), "{err:#}");
    let text = format!("{err:#}");
    assert!(
        text.contains("antigravity.run_without_guard_proof_i_understand_the_risk"),
        "{text}"
    );
    assert!(
        setup
            .order()
            .iter()
            .all(|line| !line.contains("SessionStart")),
        "the task line never went out"
    );
    assert_eq!(setup.order().len(), 1, "only the proof line was written");
}

#[test]
fn plan_reads_the_plan_file_the_fake_agy_wrote() {
    let setup = Setup::new("plan");
    let ws = setup.ws();
    let plan = setup.agent(None).plan(&issue(), &ws).expect("plan");
    assert_eq!(plan.path, ws.plan_path());
    assert_eq!(plan.open_steps, 1);
    assert!(read(&ws.plan_path()).contains("- [ ] one step"));
    assert!(
        plan.usage.input > 0,
        "the plan's usage is read from the stream"
    );
}

#[test]
fn a_logged_out_fake_agy_is_stopped_early_with_the_auth_message() {
    let setup = Setup::new("logged_out");
    let started = Instant::now();
    let err = setup
        .agent(None)
        .execute(&plan(), &setup.ws())
        .expect_err("a logged-out agy does no work");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "stopped after {:?}",
        started.elapsed()
    );
    let text = format!("{err:#}");
    assert!(text.contains("Antigravity is not signed in"), "{text}");
    assert!(text.contains(&setup.root.display().to_string()), "{text}");
}

#[test]
fn a_limit_ends_execute_as_limit_with_the_quota_reset() {
    let setup = Setup::new("limit");
    let execution = setup
        .agent(Some("gemini-3.8-flash-low"))
        .execute(&plan(), &setup.ws())
        .expect("a limit is an outcome");
    let at = chrono::DateTime::parse_from_rfc3339(FAKE_RESET).expect("the fake's reset");
    let reset = ResetAt {
        text: FAKE_RESET.to_owned(),
        at: Some(at),
    };
    assert_eq!(execution.outcome, Outcome::Limit(Some(reset)));
}

#[test]
fn execute_writes_the_agy_version_to_the_run_log() {
    let setup = Setup::new("done");
    setup
        .agent(None)
        .execute(&plan(), &setup.ws())
        .expect("execute");
    let log = setup.run_log();
    assert_eq!(
        log.lines().next(),
        Some(format!("agy version: {FAKE_VERSION}").as_str()),
        "the version heads the log:\n{log}"
    );
}

#[test]
fn a_workspace_hooks_file_is_named_in_the_run_log() {
    let setup = Setup::new("done");
    let hooks = setup.repo.join(".agents").join("hooks.json");
    fs::create_dir_all(hooks.parent().expect("a parent")).expect(".agents");
    fs::write(&hooks, "{}").expect("hooks.json");
    setup
        .agent(None)
        .execute(&plan(), &setup.ws())
        .expect("execute");
    let log = setup.run_log();
    let line = format!("workspace hooks file: {}", hooks.display());
    assert!(log.lines().any(|l| l == line), "{line} not in:\n{log}");
}

fn is_unproven(cause: &(dyn std::error::Error + 'static)) -> bool {
    cause.is::<HooksUnproven>()
}

/// A plan then an execute in one run folder.
fn plan_then_execute(setup: &Setup) {
    let agent = setup.agent(None);
    let ws = setup.ws();
    fs::write(setup.root.join("scenario"), "plan").expect("scenario");
    agent.plan(&issue(), &ws).expect("plan");
    fs::write(setup.root.join("scenario"), "done").expect("scenario");
    agent.execute(&plan(), &ws).expect("execute");
}

#[test]
fn each_session_of_a_run_keeps_its_own_log() {
    let setup = Setup::new("plan");
    plan_then_execute(&setup);
    for n in [1, 2] {
        let log = read(&setup.run_dir.join(format!("antigravity-{n}.log")));
        assert!(log.starts_with("agy version:"), "log {n}: {log:?}");
    }
}

#[test]
fn each_session_of_a_run_keeps_its_own_status_file() {
    let setup = Setup::new("plan");
    plan_then_execute(&setup);
    for n in [1, 2] {
        let status = read(&setup.run_dir.join(format!("antigravity-{n}.status.jsonl")));
        assert_eq!(
            status.matches("SessionStart").count(),
            1,
            "status {n}: {status:?}"
        );
    }
}
