use super::*;
use crate::outcome::fold;

const DONE_RUN: &str = include_str!("../../tests/fixtures/done.jsonl");
const RESUME: &str = include_str!("../../tests/fixtures/resume.jsonl");
const THINKING: &str = include_str!("../../tests/fixtures/thinking.jsonl");

#[test]
fn usage_sums_every_done_agent_response() {
    let usage = usage(&fold(DONE_RUN), Some("gemini-3.8-flash-low"));
    assert_eq!(usage.input, 8_325 + 8_451 + 8_579);
    assert_eq!(usage.input, 25_355);
    assert_eq!(usage.output, 50 + 38 + 73);
    assert_eq!(usage.model.as_deref(), Some("gemini-3.8-flash-low"));
}

#[test]
fn usage_ignores_active_agent_responses_and_other_steps() {
    let stream = concat!(
        r#"{"event":"step_update","step_update":{"state":"ACTIVE","#,
        r#""step_type":"agent_response","usage":{"input_tokens":999}}}"#,
        "\n",
        r#"{"event":"step_update","step_update":{"state":"DONE","#,
        r#""step_type":"tool","usage":{"input_tokens":999}}}"#,
    );
    assert_eq!(usage(&fold(stream), None).input, 0);
}

#[test]
fn usage_ignores_result_usage_on_a_resumed_conversation() {
    let usage = usage(&fold(RESUME), None);
    assert_eq!(usage.input, 8_835);
    assert_ne!(usage.input, 34_107);
    assert_eq!(usage.output, 4);
}

#[test]
fn output_includes_thinking_and_cache_creation_is_zero() {
    let usage = usage(&fold(THINKING), Some("gemini-3.8-flash-low"));
    assert_eq!(usage.output, 648);
    assert_eq!(usage.input, 13_731);
    assert_eq!(usage.cache_read, 14_386);
    assert_eq!(usage.cache_creation, 0);
}

#[test]
fn a_run_without_model_records_no_model_and_the_conversation_id() {
    let fold = fold(RESUME);
    let usage = usage(&fold, None);
    assert_eq!(usage.model, None);
    assert_eq!(
        fold.conversation_id.as_deref(),
        Some("00000000-0000-4000-8000-000000000002")
    );
}
