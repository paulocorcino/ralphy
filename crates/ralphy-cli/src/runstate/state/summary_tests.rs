//! The run-level events that write the card's terminal summary.

use super::*;

#[test]
fn run_skipped_makes_its_reason_the_summary_and_changes_no_issue() {
    let reason = "skipped: run in progress since 2026-07-19 10:00:00, pid 4242";
    let mut state = RunState::new("t", 0);
    let issues_before = state.issues.clone();
    state.apply(RunEvent::RunSkipped {
        reason: reason.into(),
    });
    assert_eq!(state.final_summary.as_deref(), Some(reason));
    assert_eq!(state.issues, issues_before);
}

fn run_finished(outcome: &str) -> RunEvent {
    RunEvent::RunFinished {
        outcome: outcome.into(),
        issues_done: 0,
        issues_skipped: 0,
        issues_total: 0,
        issues_blocked: 0,
        issues_hitl: 0,
        issues: serde_json::Value::Null,
        up: 0,
        cr: 0,
        cw: 0,
        out: 0,
        duration_s: 0,
    }
}

#[test]
fn only_a_no_work_finish_writes_a_summary() {
    let mut empty = RunState::new("t", 0);
    empty.apply(run_finished("no_work"));
    assert_eq!(
        empty.final_summary.as_deref(),
        Some("no open issues to process")
    );
    for outcome in ["completed", "stopped", "deadline"] {
        let mut state = RunState::new("t", 0);
        state.apply(run_finished(outcome));
        assert_eq!(state.final_summary, None, "{outcome}");
    }
}
