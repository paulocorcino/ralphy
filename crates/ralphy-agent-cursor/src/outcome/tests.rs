use super::*;

const INIT: &str = concat!(
    r#"{"type":"system","subtype":"init","apiKeySource":"login","cwd":"C:\\Dev\\FinCal","#,
    r#""session_id":"868f1553-01ac-4335-89c6-6c1f101d6009","model":"Auto","#,
    r#""permissionMode":"force"}"#
);

const PERMISSION_DENIED: &str = include_str!("../../fixtures/permission-denied-2026-07-20.jsonl");
const TOOL_FAILURE: &str = include_str!("../../fixtures/tool-failure-2026-07-20.jsonl");
const KILLED: &str = include_str!("../../fixtures/killed-2026-07-20.jsonl");
const KILLED_ERR: &str = include_str!("../../fixtures/killed-2026-07-20.err");
const INTERRUPTED_STREAM: &str = include_str!("../../fixtures/interrupted-2026-07-20.jsonl");
const INTERRUPTED_ERR: &str = include_str!("../../fixtures/interrupted-2026-07-20.err");
const PREFLIGHT_REJECTION: &str =
    include_str!("../../fixtures/preflight-rejection-2026-07-20.jsonl");
const PREFLIGHT_REJECTION_ERR: &str =
    include_str!("../../fixtures/preflight-rejection-2026-07-20.err");
/// The two quota refusals that blocked the live execute pass on 2026-07-21.
///
/// PROVENANCE, because it is not the same as the fixtures above: these were
/// recovered from the vendor's session store
/// (`~/.cursor/projects/<slug>/agent-transcripts/`), the run's stdout having
/// not been captured. That store serialises the conversational records
/// differently from the stream — bare `{"role","message"}` objects with no
/// `type` field — and the fold skips those either way. What both files share
/// with the stream, byte for byte, is the terminal record under test.
const USAGE_LIMIT: &str = include_str!("../../fixtures/usage-limit-2026-07-21.jsonl");
const USAGE_LIMIT_MIDTURN: &str =
    include_str!("../../fixtures/usage-limit-midturn-2026-07-21.jsonl");
/// The quota stop AS MEASURED: the vendor's bare `ActionRequiredError:` stderr
/// prose in the MERGED log, with NO terminal record at all. Captured live on
/// 2026-07-22 during the #251 capstone — the `cursor.log` of a plan run whose
/// child hit the Free-tier limit, byte-identical to the direct `agent -p` stderr.
/// The record-shape fixtures above were D13's pre-implementation guess; THIS is
/// the shape the run found, and the fold must read it the same way.
const USAGE_LIMIT_STDERR: &str = include_str!("../../fixtures/usage-limit-stderr-2026-07-22.log");

fn envelope(subtype: &str, is_error: bool, result: &str) -> String {
    serde_json::json!({
        "type": "result",
        "subtype": subtype,
        "is_error": is_error,
        "duration_ms": 25253,
        "result": result,
        "session_id": "868f1553-01ac-4335-89c6-6c1f101d6009",
        "usage": {"inputTokens": 19264, "outputTokens": 1303,
                  "cacheReadTokens": 5248, "cacheWriteTokens": 0}
    })
    .to_string()
}

#[test]
fn clean_success_carries_the_sentinel_as_the_last_line_of_result() {
    let stdout = format!(
        "{INIT}\n{}\n",
        envelope("success", false, "all green\nRALPHY_DONE_EXIT")
    );
    let fold = fold_cursor_stream(&stdout);
    assert!(fold.saw_envelope);
    assert_eq!(fold.subtype.as_deref(), Some("success"));
    assert!(!fold.is_error);
    assert!(
        fold.final_text.ends_with("RALPHY_DONE_EXIT"),
        "{:?}",
        fold.final_text
    );
    assert_eq!(
        fold.session_id.as_deref(),
        Some("868f1553-01ac-4335-89c6-6c1f101d6009")
    );
    assert_eq!(
        classify_cursor_outcome(&fold, true, false, true, Some(0)),
        Outcome::Done
    );
}

/// D6/D3: the permission-denied fixture's envelope is a genuine `subtype:
/// "success"` stream whose text carries the spike's decoy token, not Ralphy's
/// `DONE_SENTINEL` — so committing something is not the same as buying a green
/// close.
#[test]
fn a_permission_denied_run_is_green_and_names_the_blocked_command() {
    let fold = fold_cursor_stream(PERMISSION_DENIED);
    assert_eq!(fold.subtype.as_deref(), Some("success"));
    assert!(!fold.is_error);
    assert_eq!(
        fold.denied_tool_calls,
        vec!["git status --short".to_string()]
    );
    let note = fold
        .degraded_note()
        .expect("a blocked command must be visible");
    assert!(note.contains("git status --short"), "{note}");
}

/// Companion to the pin above, over the SAME fixture: a real success envelope
/// whose text never carries `DONE_SENTINEL` must not classify `Done`, even with
/// `committed = true` — commits alone never buy a green close.
#[test]
fn an_envelope_without_the_sentinel_is_not_done() {
    let fold = fold_cursor_stream(PERMISSION_DENIED);
    assert_ne!(
        classify_cursor_outcome(&fold, true, false, true, Some(0)),
        Outcome::Done,
        "commits alone never buy a green close"
    );
}

/// D3 rule 1: the discriminator is inside the tool record, and the run reports
/// success regardless. The outcome must not read it — proved by clearing the
/// tool-call vectors and reclassifying to the SAME `Outcome`.
#[test]
fn a_failed_tool_call_does_not_change_the_outcome() {
    let mut fold = fold_cursor_stream(TOOL_FAILURE);
    assert!(
        fold.failed_tool_calls[0].contains("exit 42"),
        "{:?}",
        fold.failed_tool_calls
    );
    assert_eq!(fold.subtype.as_deref(), Some("success"));
    assert!(!fold.is_error, "a failed tool call is not a failed run");
    assert!(
        fold.degraded_note().is_some(),
        "but it IS surfaced as degraded"
    );
    let before = classify_cursor_outcome(&fold, true, false, true, Some(0));
    fold.failed_tool_calls.clear();
    fold.denied_tool_calls.clear();
    let after = classify_cursor_outcome(&fold, true, false, true, Some(0));
    assert_eq!(
        before, after,
        "the outcome never reads the tool-call vectors"
    );
}

/// D3 rule 3: partial records, no envelope, and an EMPTY stderr — the one case
/// where stderr says nothing at all, so an adapter classifying on stderr alone
/// sees a silent success. Here the missing envelope is what fails it.
#[test]
fn a_truncated_stream_has_records_but_no_envelope() {
    let fold = fold_cursor_stream(KILLED);
    assert!(!fold.saw_envelope, "the run died before the envelope");
    assert!(
        !fold.saw_no_records(),
        "records DID arrive — not a preflight rejection"
    );
    assert_eq!(
        classify_cursor_outcome(&fold, false, false, false, Some(1)),
        Outcome::Stuck
    );
    assert!(KILLED_ERR.is_empty(), "the empty-stderr half of D3 rule 3");
}

/// D3 rule 2: zero records + exit 1. Distinguishable from a truncated run (the
/// killed fixture) by the record count, which is why the fold tracks "saw
/// anything at all".
#[test]
fn a_preflight_rejection_has_zero_records() {
    let fold = fold_cursor_stream(PREFLIGHT_REJECTION);
    assert!(!fold.saw_envelope);
    assert!(fold.saw_no_records(), "no record of any kind arrived");
    assert!(
        !fold_cursor_stream(KILLED).saw_no_records(),
        "the killed fixture DID see records — that's the discriminator"
    );
    assert_eq!(
        classify_cursor_outcome(&fold, false, false, false, Some(1)),
        Outcome::Stuck
    );
    assert!(
        PREFLIGHT_REJECTION_ERR.contains("Workspace directory does not exist"),
        "{PREFLIGHT_REJECTION_ERR}"
    );
}

/// Never reproduced, therefore handled defensively (D3): an unknown `subtype`
/// is NOT success, even alongside a sentinel and a clean exit.
#[test]
fn an_unknown_subtype_is_not_success() {
    let stdout = format!(
        "{INIT}\n{}\n",
        envelope("weird", false, "all green\nRALPHY_DONE_EXIT")
    );
    let fold = fold_cursor_stream(&stdout);
    assert_eq!(fold.subtype.as_deref(), Some("weird"));
    assert_ne!(
        classify_cursor_outcome(&fold, true, false, true, Some(0)),
        Outcome::Done
    );

    // `is_error: true` — the other never-reproduced shape — fails the same way.
    let errored = format!(
        "{INIT}\n{}\n",
        envelope("success", true, "all green\nRALPHY_DONE_EXIT")
    );
    assert_ne!(
        classify_cursor_outcome(&fold_cursor_stream(&errored), true, false, true, Some(0)),
        Outcome::Done
    );
}

/// A vendor refusal that names its own cause must not arrive as a mute stop.
/// Measured: the account's allowance ran out and the vendor said so, in a
/// well-formed terminal record — not in the `ActionRequiredError` stderr prose
/// ADR-0042 anticipated. Reading `status` is the whole difference between
/// "Stuck, no idea" and the vendor's own sentence in the run log.
#[test]
fn a_turn_ended_in_error_carries_the_vendors_reason() {
    let fold = fold_cursor_stream(USAGE_LIMIT);
    assert!(fold.is_error, "status: error is an error");
    assert!(
        fold.vendor_error
            .as_deref()
            .is_some_and(|m| m.contains("usage limit")),
        "{:?}",
        fold.vendor_error
    );
    assert!(
        !fold.saw_no_records(),
        "the turn ended on a record — this is a refusal, not a preflight rejection"
    );
    assert_ne!(
        classify_cursor_outcome(&fold, false, false, false, Some(1)),
        Outcome::Done
    );
}

/// The same refusal arriving mid-turn, after the agent had already merged
/// `origin/main` through its shell tool. Nothing about the tool work makes the
/// stop any less explicit, and no `result` envelope ever comes.
#[test]
fn a_midturn_refusal_reads_the_same_as_a_bare_one() {
    let fold = fold_cursor_stream(USAGE_LIMIT_MIDTURN);
    assert!(!fold.saw_envelope, "the stream ends on `turn_ended`");
    assert_eq!(
        fold.vendor_error,
        fold_cursor_stream(USAGE_LIMIT).vendor_error,
        "one refusal, one sentence, wherever in the turn it lands"
    );
    // Committed work does not buy a green close for a run the vendor refused
    // to finish: the sentinel never arrived.
    assert_ne!(
        classify_cursor_outcome(&fold, false, false, true, Some(1)),
        Outcome::Done
    );
}

/// #266: a quota stop classifies as `Limit(None)` — no reset hint is ever
/// published, so ADR-0030's synthetic cadence schedules the resumption.
#[test]
fn a_quota_stop_is_a_limit_with_no_reset_hint() {
    let fold = fold_cursor_stream(USAGE_LIMIT);
    assert!(
        cursor_limit_note(&fold)
            .as_deref()
            .is_some_and(|m| m.contains("You've hit your usage limit")),
        "{:?}",
        cursor_limit_note(&fold)
    );
    assert_eq!(
        classify_cursor_outcome(&fold, false, false, false, Some(1)),
        Outcome::Limit(None)
    );

    let midturn = fold_cursor_stream(USAGE_LIMIT_MIDTURN);
    assert!(
        cursor_limit_note(&midturn)
            .as_deref()
            .is_some_and(|m| m.contains("You've hit your usage limit")),
        "{:?}",
        cursor_limit_note(&midturn)
    );
    assert_eq!(
        classify_cursor_outcome(&midturn, false, false, true, Some(1)),
        Outcome::Limit(None)
    );
}

/// #266: the note names the partial work when the call already committed,
/// and stays silent about it when nothing landed.
#[test]
fn a_midturn_quota_stop_names_the_partial_work() {
    let midturn = fold_cursor_stream(USAGE_LIMIT_MIDTURN);
    let note = limit_stop_note(&midturn, Some("abc1234..def5678"))
        .expect("a quota stop must produce a note");
    assert!(note.contains("abc1234..def5678"), "{note}");
    assert!(note.contains("kept on the branch"), "{note}");

    let bare = fold_cursor_stream(USAGE_LIMIT);
    let note = limit_stop_note(&bare, None).expect("a quota stop must produce a note");
    assert!(note.contains("usage limit"), "{note}");
    assert!(!note.contains("kept on the branch"), "{note}");
}

/// #251: the quota stop AS IT ACTUALLY ARRIVES — a bare `ActionRequiredError:`
/// stderr line, no `turn_ended`, no envelope. Before the capstone this shape
/// folded to nothing, so the plan path reported "produced no plan" and execute
/// reported `Stuck`; both buried the vendor's own sentence. It must read as
/// `Limit(None)`, and carry the SAME sentence as the record shape.
#[test]
fn the_bare_stderr_quota_shape_is_a_limit() {
    let fold = fold_cursor_stream(USAGE_LIMIT_STDERR);
    assert!(
        !fold.saw_turn_end && !fold.saw_envelope,
        "the measured shape carries NO terminal record"
    );
    assert!(
        fold.vendor_error
            .as_deref()
            .is_some_and(|m| m.contains("usage limit")),
        "the bare stderr line must carry the vendor's sentence: {:?}",
        fold.vendor_error
    );
    assert!(
        cursor_limit_note(&fold)
            .as_deref()
            .is_some_and(|m| m.contains("You've hit your usage limit")),
        "{:?}",
        cursor_limit_note(&fold)
    );
    assert_eq!(
        fold.vendor_error,
        fold_cursor_stream(USAGE_LIMIT).vendor_error,
        "one refusal, one sentence — bare-stderr and record shapes fold alike"
    );
    assert_eq!(
        classify_cursor_outcome(&fold, false, false, false, Some(1)),
        Outcome::Limit(None)
    );
}

/// #251: the bare-line capture is gated to the LIMIT classes. A bare
/// `ActionRequiredError:` that is NOT a quota — an entitlement refusal, which
/// `model_refusal_stop` owns — must not fold to a `vendor_error`, or it would
/// masquerade as a quota stop and schedule a pointless wait.
#[test]
fn a_bare_non_limit_action_required_is_not_a_limit() {
    let stream = format!("{INIT}\nActionRequiredError: Named models unavailable on your plan\n");
    let fold = fold_cursor_stream(&stream);
    assert_eq!(fold.vendor_error, None, "not a quota class → not captured");
    assert_eq!(cursor_limit_note(&fold), None);
}

/// #266: `vendor_error` is the only carrier — a working run whose transcript
/// merely QUOTES the sentence must not classify as a limit.
#[test]
fn a_quoted_quota_sentence_in_a_green_transcript_is_not_a_limit() {
    let stdout = format!(
        "{INIT}\n{}\n",
        envelope(
            "success",
            false,
            "You've hit your usage limit\nRALPHY_DONE_EXIT"
        )
    );
    let fold = fold_cursor_stream(&stdout);
    assert_eq!(cursor_limit_note(&fold), None);
    assert_eq!(
        classify_cursor_outcome(&fold, true, false, true, Some(0)),
        Outcome::Done
    );
}

/// #266: a quota stop is distinct from #245's entitlement refusal and from
/// Ralphy's own budget/idle-watchdog stop.
#[test]
fn a_quota_stop_is_not_an_entitlement_refusal_nor_a_watchdog_stop() {
    const ENTITLEMENT: &str = include_str!("../../fixtures/model-entitlement-2026-07-21.err");
    assert_eq!(cursor_limit_note(&fold_cursor_stream(ENTITLEMENT)), None);

    let fold = fold_cursor_stream(INTERRUPTED_STREAM);
    assert_eq!(
        classify_cursor_outcome(&fold, false, false, false, Some(130)),
        Outcome::Timeout
    );
}

/// A `turn_ended` that says `success` is not an error, and — since the ladder
/// keys success off the `result` envelope — it does not manufacture one either.
#[test]
fn a_successful_turn_end_is_not_an_error() {
    let stdout = r#"{"type":"turn_ended","status":"success"}"#;
    let fold = fold_cursor_stream(stdout);
    assert!(!fold.is_error);
    assert_eq!(fold.vendor_error, None);
    assert!(!fold.saw_envelope);
    assert_ne!(
        classify_cursor_outcome(&fold, true, false, true, Some(0)),
        Outcome::Done,
        "a turn end without the envelope is still a truncated stream"
    );
}

/// ADR-0038: exit 130 is what Ralphy's own budget and idle watchdogs produce.
/// Reporting that as `Stuck` would blame the agent for a stop Ralphy chose.
#[test]
fn an_interrupt_is_reported_as_interrupted() {
    let fold = fold_cursor_stream(INTERRUPTED_STREAM);
    let outcome = classify_cursor_outcome(&fold, false, false, false, Some(130));
    assert_eq!(
        outcome,
        Outcome::Timeout,
        "`Aborting operation...` is a stop, not a crash"
    );
    assert_eq!(
        INTERRUPTED_ERR.trim(),
        "Aborting operation...",
        "{INTERRUPTED_ERR:?}"
    );
    // A hard kill (no exit code at all, empty stderr) stays Stuck.
    assert_eq!(
        classify_cursor_outcome(&fold, false, false, false, None),
        Outcome::Stuck
    );
}

#[test]
fn a_clean_run_has_no_degraded_note() {
    let stdout = format!(
        "{INIT}\n{}\n",
        envelope("success", false, "RALPHY_DONE_EXIT")
    );
    assert!(fold_cursor_stream(&stdout).degraded_note().is_none());
}

mod pins;
