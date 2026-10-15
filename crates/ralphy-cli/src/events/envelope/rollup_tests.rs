use super::*;

fn ctx() -> EventCtx {
    EventCtx {
        source: "ralphy/o/r".to_string(),
        runid: "01RUNIDRUNIDRUNIDRUNID".to_string(),
        emitter: json!({}),
        git: json!({}),
    }
}

fn finished(rollup: Value, state: &RunState) -> Value {
    let ev = RunEvent::RunFinished {
        outcome: "completed".into(),
        issues_done: 0,
        issues_skipped: 1,
        issues_total: 1,
        issues_blocked: 0,
        issues_hitl: 0,
        issues: rollup,
        up: 0,
        cr: 0,
        cw: 0,
        out: 0,
        duration_s: 0,
    };
    runevent_to_cloudevent(&ev, &ctx(), state).expect("mapped")
}

/// A skipped entry of the run's own rollup keeps its `kind` and `blocked_by`;
/// the title comes from the fold.
#[test]
fn run_finished_rollup_keeps_the_kind_and_blockers_of_a_skip() {
    let mut state = RunState::new("t", 1);
    state.apply(RunEvent::QueueBuilt {
        count: 1,
        order: vec![2],
        stop_before: None,
        issues: json!([{ "number": 2, "title": "two" }]),
        assignee_filter: None,
        scope: None,
    });
    let rollup = json!([
        { "number": 2, "status": "skipped", "kind": "blocked_by", "blocked_by": [1] }
    ]);
    let v = finished(rollup, &state);
    assert_eq!(
        v["data"]["issues"],
        json!([
            { "number": 2, "title": "two", "status": "skipped", "kind": "blocked_by", "blocked_by": [1] }
        ])
    );
}

/// Without a rollup, an issue the fold still sees in flight has no final
/// status, so it is left out of `data.issues`.
#[test]
fn run_finished_fold_leaves_out_an_issue_in_flight() {
    let mut state = RunState::new("t", 1);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "one".into(),
    });
    let v = finished(Value::Null, &state);
    assert_eq!(v["data"]["issues"], json!([]));
}
