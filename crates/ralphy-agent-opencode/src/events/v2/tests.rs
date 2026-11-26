use ralphy_adapter_support::{failed_run_stop, RunFacts, RunStop};
use ralphy_core::{Agent, Execution, Outcome, Plan, Workspace};
use ralphy_proc_util::opencode::OpenCodeMajor::{self, V2};

use crate::events::{
    is_opencode_api_degraded, is_opencode_auth_error, parse_opencode_events,
    OPENCODE_AUTH_ERROR_MSG,
};
use crate::outcome::{classify_opencode_outcome, CallRead};
use crate::usage::session_id_from_stream;
use crate::OpenCodeAgent;

/// A file under `tests/fixtures/`.
pub(crate) fn fixture(path: &str) -> String {
    let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{}: {e}", full.display()))
}

/// How one call ends: the shared auth stop, or an outcome.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    AuthStop,
    Ends(Outcome),
}

/// The end of one call that wrote `stdout` and `log` and exited as given,
/// with no commit, by the precedence of the execute phase.
pub(crate) fn verdict(major: OpenCodeMajor, stdout: &str, log: &str, exited: bool) -> Verdict {
    let facts = RunFacts {
        exited_cleanly: exited,
        timed_out: false,
        stopped: false,
    };
    if failed_run_stop(&facts, log, is_opencode_auth_error) == Some(RunStop::Auth) {
        return Verdict::AuthStop;
    }
    let read = CallRead::new(major, stdout, log);
    Verdict::Ends(classify_opencode_outcome(
        exited,
        false,
        false,
        read.reported(),
    ))
}

/// One 2.x `error` event of `error_type` with `message`.
fn error_line(error_type: &str, message: &str) -> String {
    serde_json::json!({
        "type": "error",
        "timestamp": 1,
        "sessionID": "ses_1",
        "error": {"type": error_type, "message": message, "status": 429},
    })
    .to_string()
}

const RETRY_LINE: &str = concat!(
    r#"{"type":"retry","timestamp":1,"sessionID":"ses_1","messageID":"msg_1","attempt":1,"#,
    r#""nextRetryAt":2,"error":{"type":"provider.rate-limit","message":"Rate limit exceeded","#,
    r#""status":429}}"#,
);

/// An execute on 2.x whose child writes `stdout` and exits 1.
fn failed_2x_execute(stdout: String) -> anyhow::Result<Execution> {
    let temp = tempfile::tempdir().expect("a scratch folder");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("a scratch repo");
    let ws = Workspace::new(&repo);
    let agent = OpenCodeAgent::new(None, temp.path().join("run"))
        .with_version(|| Ok(crate::command::tests::located(V2)))
        .with_run_hook(move || {
            Ok(ralphy_adapter_support::HeadlessRun {
                stdout: stdout.clone(),
                log: stdout.clone(),
                exited_cleanly: false,
                timed_out: false,
                idle_killed: false,
                stopped: false,
                exit_code: Some(1),
            })
        });
    let plan = Plan {
        path: ws.plan_path(),
        open_steps: 1,
        recommended_model: None,
        usage: ralphy_core::Usage::default(),
        session_id: None,
    };
    agent.execute(&plan, &ws)
}

#[test]
fn v2_execute_fixtures_end_done() {
    for name in [
        "v2/execute-win.jsonl",
        "v2/execute-wsl.jsonl",
        "v2/execute-service-win.jsonl",
        "v2/execute-service-wsl.jsonl",
    ] {
        let stdout = fixture(name);
        let (text, _) = parse_opencode_events(V2, &stdout);
        assert!(!text.trim().is_empty(), "{name}: no text");
        assert!(
            ralphy_adapter_support::done_sentinel(&text),
            "{name}: no done sentinel"
        );
        assert!(
            session_id_from_stream(&stdout).is_some(),
            "{name}: no session id"
        );
        assert_eq!(
            verdict(V2, &stdout, &stdout, true),
            Verdict::Ends(Outcome::Done),
            "{name}"
        );
    }
}

#[test]
fn a_2x_retry_event_is_not_an_error() {
    let stdout = format!("{RETRY_LINE}\n");
    let (_, saw_error) = parse_opencode_events(V2, &stdout);
    assert!(!saw_error);
}

#[test]
fn a_2x_retry_event_is_a_degraded_line() {
    assert!(is_opencode_api_degraded(V2, RETRY_LINE));
    assert!(!is_opencode_api_degraded(
        V2,
        &error_line("provider.other-thing", "boom")
    ));
}

#[test]
fn the_2x_auth_fixture_is_the_auth_error() {
    let err = failed_2x_execute(fixture("v2/auth-bad-key-win.jsonl"))
        .expect_err("a refused key stops the run");
    assert!(
        format!("{err:#}").contains(OPENCODE_AUTH_ERROR_MSG),
        "{err:#}"
    );
}

#[test]
fn the_2x_no_route_fixture_stops_with_both_causes() {
    let err = failed_2x_execute(fixture("v2/no-login-win.jsonl"))
        .expect_err("a model with no route stops the run");
    let text = format!("{err:#}");
    assert!(text.contains("opencode auth login"), "{text}");
    assert!(text.contains("model id"), "{text}");
    assert!(!text.contains(OPENCODE_AUTH_ERROR_MSG), "{text}");
}

#[test]
fn every_2x_limit_type_is_an_unschedulable_limit() {
    for error_type in [
        "provider.quota",
        "provider.free-limit",
        "provider.go-limit",
        "provider.budget-limit",
        "provider.model-budget-limit",
        "provider.chatgpt-limit",
        "provider.rate-limit",
    ] {
        let stdout = error_line(error_type, "Please recharge.");
        assert_eq!(
            verdict(V2, &stdout, &stdout, false),
            Verdict::Ends(Outcome::Limit(None)),
            "{error_type}"
        );
    }
}

#[test]
fn the_2x_zai_quota_fixture_is_a_limit() {
    let stdout = fixture("v2/quota-zai-win.jsonl");
    assert_eq!(
        verdict(V2, &stdout, &stdout, false),
        Verdict::Ends(Outcome::Limit(None))
    );
}

#[test]
fn an_unknown_2x_error_type_is_stuck() {
    let stdout = error_line("provider.other-thing", "Something went wrong.");
    assert_eq!(
        verdict(V2, &stdout, &stdout, false),
        Verdict::Ends(Outcome::Stuck)
    );
}

#[test]
fn an_unknown_2x_type_with_limit_words_is_a_limit() {
    let stdout = error_line("provider.other-thing", "Usage limit reached for today.");
    assert_eq!(
        verdict(V2, &stdout, &stdout, false),
        Verdict::Ends(Outcome::Limit(None))
    );
}

#[test]
fn two_x_info_lines_on_stderr_are_not_a_limit() {
    let stdout = fixture("v2/plan-win.jsonl");
    let log = format!("{stdout}{}", fixture("v2/standalone-stderr-win.log"));
    assert_eq!(
        verdict(V2, &stdout, &log, true),
        Verdict::Ends(Outcome::Done)
    );
}
