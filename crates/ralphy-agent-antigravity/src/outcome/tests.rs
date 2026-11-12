use super::*;
use ralphy_adapter_support::classify;
use ralphy_core::Outcome;

const DONE: &str = include_str!("../../tests/fixtures/done.jsonl");
const TWO_TURNS: &str = include_str!("../../tests/fixtures/two_turns.jsonl");
const DENIED: &str = include_str!("../../tests/fixtures/denied.jsonl");
const PRINT_TIMEOUT: &str = include_str!("../../tests/fixtures/print_timeout.jsonl");
const PRINT_TIMEOUT_EXIT_ZERO: &str =
    include_str!("../../tests/fixtures/print_timeout_exit_zero.jsonl");
const QUOTA: &str = include_str!("../../tests/fixtures/quota.jsonl");

const TIMEOUT_STDERR: &str = concat!(
    "error: print timeout after 30s while the agent was waiting on 1 background task(s)\n",
    "AGY_ERROR: {\"short_error\":\"print timeout after 30s\",\"status\":\"DEADLINE_EXCEEDED\",",
    "\"retryable\":false,\"reason\":\"print_timeout\",\"tasks\":[{\"id\":",
    "\"00000000-0000-4000-8000-000000000001/task-2\"}]}\n",
);

const UTF8_STDERR: &str = concat!(
    "error: agent executor error: generating and executing: proto: field ",
    "google.protobuf.Value.string_value contains invalid UTF-8\n",
    "AGY_ERROR: {\"short_error\":\"agent executor error\",\"status\":\"UNKNOWN\",",
    "\"error_code\":2,\"code_kind\":\"grpc\",\"retryable\":false}\n",
);

fn exit(code: i32) -> RunEnd {
    RunEnd {
        code: Some(code),
        ..RunEnd::default()
    }
}

fn outcome(capture: &str, stderr: &str, end: RunEnd) -> Outcome {
    classify(completion_signals(&fold(capture), stderr, end))
}

#[test]
fn the_fold_takes_the_last_result_line() {
    let fold = fold(TWO_TURNS);
    let result = fold.result.expect("the capture ends with a result line");
    assert_eq!(
        result.response, "RALPHY_DONE_EXIT\n",
        "the task turn, not the proof turn"
    );
    assert_eq!(
        fold.conversation_id.as_deref(),
        Some("00000000-0000-4000-8000-000000000001")
    );
    assert_eq!(fold.steps.len(), 6, "every step_update of both turns");
}

#[test]
fn the_sentinel_in_the_last_response_line_is_done() {
    let signals = completion_signals(&fold(DONE), "", exit(0));
    assert!(signals.done);
    assert!(signals.exited_ok);
    assert!(!signals.errored);
    assert_eq!(classify(signals), Outcome::Done);

    let not_last = DONE.replace(
        "RALPHY_DONE_EXIT\\n\"",
        "RALPHY_DONE_EXIT\\nThen I stopped.\\n\"",
    );
    assert!(
        !completion_signals(&fold(&not_last), "", exit(0)).done,
        "the sentinel counts only on the last line"
    );
}

#[test]
fn exit_zero_with_a_finished_reason_is_not_exited_ok() {
    let signals = completion_signals(&fold(PRINT_TIMEOUT_EXIT_ZERO), "", exit(0));
    assert!(!signals.exited_ok);
    assert!(signals.timed_out, "print_timeout is a timeout");
    assert_eq!(classify(signals), Outcome::Timeout);
}

#[test]
fn denied_actions_mark_the_run_errored() {
    let with_sentinel = DENIED.replace("\"response\":\"\"", "\"response\":\"RALPHY_DONE_EXIT\\n\"");
    let signals = completion_signals(&fold(&with_sentinel), "", exit(0));
    assert!(signals.done);
    assert!(signals.errored);
    assert_eq!(classify(signals), Outcome::Stuck);
}

#[test]
fn a_stream_with_no_result_line_is_errored() {
    let cut = DONE
        .lines()
        .filter(|line| !line.contains("\"event\":\"result\""))
        .collect::<Vec<_>>()
        .join("\n");
    let fold = fold(&cut);
    assert!(fold.result.is_none());
    let signals = completion_signals(&fold, "", exit(0));
    assert!(signals.errored);
    assert_eq!(classify(signals), Outcome::Stuck);
}

#[test]
fn limit_patterns_classify_a_quota_error_as_limit() {
    let rows = [
        concat!(
            "AGY_ERROR: {\"short_error\":\"rpc error\",\"status\":\"RESOURCE_EXHAUSTED\",",
            "\"code_kind\":\"grpc\",\"retryable\":false}"
        ),
        "error: individual quota reached",
        concat!(
            "AGY_ERROR: {\"short_error\":\"Too Many Requests\",\"error_code\":429,",
            "\"code_kind\":\"http\",\"retryable\":false}"
        ),
        "error: rate limit reached for requests",
        "error: MODEL_CAPACITY_EXHAUSTED",
        "error: Your AI credits balance is too low to continue",
    ];
    for stderr in rows {
        assert_eq!(
            outcome(QUOTA, stderr, exit(3)),
            Outcome::Limit(None),
            "{stderr}"
        );
    }
    assert_eq!(
        outcome(PRINT_TIMEOUT, TIMEOUT_STDERR, exit(3)),
        Outcome::Timeout,
        "digits inside an id are not a 429"
    );
    assert_eq!(
        outcome(DONE, "retrying after 429 Too Many Requests\n", exit(0)),
        Outcome::Done,
        "a 429 the CLI retried does not void a run that ended well"
    );
}

#[test]
fn exit_three_with_invalid_utf8_is_a_machine_fault() {
    let hooks = Path::new("/data/antigravity-root/config/hooks.json");
    let message = machine_fault(Some(3), UTF8_STDERR, hooks).expect("exit 3 with invalid UTF-8");
    assert!(message.contains(&hooks.display().to_string()), "{message}");
    assert_eq!(machine_fault(Some(3), TIMEOUT_STDERR, hooks), None);
    assert_eq!(machine_fault(Some(1), UTF8_STDERR, hooks), None);
}
