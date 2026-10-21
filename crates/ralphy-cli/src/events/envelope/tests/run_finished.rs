use super::*;

#[test]
fn run_finished_maps_outcome_totals_without_subject() {
    // Fold a lifecycle: issue 1 closed (done), issue 2 skipped via a
    // human-return (a titled skip carrying `kind`), and seed a queue so the
    // rollup title can fall back for the skip (which never got an IssueStarted).
    let mut state = RunState::new("t", 5);
    state.apply(RunEvent::QueueBuilt {
        count: 5,
        order: vec![1, 2],
        stop_before: None,
        issues: json!([
            {"number": 1, "title": "one"},
            {"number": 2, "title": "two"},
        ]),
        assignee_filter: None,
        scope: None,
    });
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "one".into(),
    });
    state.apply(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    });
    state.apply(RunEvent::Skipped {
        number: 2,
        kind: SkipKind::HumanReturn,
        label: Some("needs-info".into()),
        blockers: vec![],
    });
    let v = map(
        RunEvent::RunFinished {
            outcome: "completed".into(),
            issues_done: 3,
            issues_skipped: 1,
            issues_total: 5,
            issues_blocked: 0,
            issues_hitl: 0,
            // No rollup — this pins the LEGACY fold-derived `data.issues` path.
            issues: Value::Null,
            up: 100,
            cr: 200,
            cw: 50,
            out: 25,
            duration_s: 412,
        },
        &state,
    );
    assert_eq!(v["type"], "dev.ralphy.run.finished");
    assert!(
        v.get("subject").is_none(),
        "run.finished has no subject: {v}"
    );
    assert_eq!(v["data"]["outcome"], "completed");
    // Scalar counts stay intact.
    assert_eq!(v["data"]["issues_done"], 3);
    assert_eq!(v["data"]["issues_skipped"], 1);
    assert_eq!(v["data"]["issues_total"], 5);
    assert_eq!(v["data"]["tokens_total"]["up"], 100);
    assert_eq!(v["data"]["tokens_total"]["out"], 25);
    assert_eq!(v["data"]["duration_s"], 412);
    // The per-issue rollup: a done entry (no kind) and a skipped entry (kind).
    assert_eq!(
        v["data"]["issues"],
        json!([
            { "number": 1, "title": "one", "status": "done" },
            { "number": 2, "title": "two", "status": "skipped", "kind": "human_return", "blocked_by": [] },
        ])
    );
    // `run.finished` carries NO agent block.
    assert!(
        v["data"].get("agent").is_none(),
        "run.finished has no agent block: {v}"
    );
}

/// Decode a `run finished` the way the live pipeline does — through the real
/// field decoder — so the test exercises the emit→decode→envelope chain rather
/// than hand-building the typed event.
fn decode_run_finished(summary: &crate::run::summary::RunSummary) -> RunEvent {
    let f = crate::runstate::EventFields {
        outcome: Some(summary.outcome.to_string()),
        issues_done: Some(summary.done),
        issues_skipped: Some(summary.skipped),
        issues_total: Some(summary.total),
        issues_blocked: Some(summary.blocked),
        issues_hitl: Some(summary.hitl),
        issues_json: Some(summary.issues_json()),
        ..Default::default()
    };
    crate::runstate::event_to_runevent("ralphy_core::emit", ralphy_core::emit::RUN_FINISHED_MSG, &f)
        .expect("the decoder knows `run finished`")
}

/// The drop-oldest case (#224): the console fold saw NOTHING, yet the envelope's
/// `data.issues[]` and the scalars beside it must still agree — because both now
/// come from the run's own summary, not from the fold.
#[test]
fn run_finished_rollup_stays_coherent_under_a_starved_fold() {
    let summary = crate::run::summary::RunSummary::from_report(
        &crate::run::summary::tests::fixture_report(),
        7,
    );
    let v = map(decode_run_finished(&summary), &RunState::default());

    let issues = v["data"]["issues"].as_array().expect("an issues array");
    assert_eq!(issues.len(), 6, "the starved fold does not truncate: {v}");

    let count = |s: &str| issues.iter().filter(|e| e["status"] == s).count() as u64;
    assert_eq!(count("done"), v["data"]["issues_done"].as_u64().unwrap());
    assert_eq!(count("done"), 1);
    assert_eq!(
        count("non_green") + count("blocked"),
        v["data"]["issues_blocked"].as_u64().unwrap()
    );
    assert_eq!(count("non_green") + count("blocked"), 1);
    assert_eq!(count("hitl"), v["data"]["issues_hitl"].as_u64().unwrap());
    assert_eq!(count("hitl"), 1);
    // Everything else (skipped ×2 + planned) is the generic skip bucket.
    let rest =
        issues.len() as u64 - count("done") - count("non_green") - count("blocked") - count("hitl");
    assert_eq!(rest, v["data"]["issues_skipped"].as_u64().unwrap());
    assert_eq!(rest, 3);
}

/// #313: the per-issue review-only count rides the rollup all the way to the
/// wire, so the operator's panel and the event agree — and rides it only when
/// non-zero (ADR-0019 evolution rule 1, additive-only).
#[test]
fn run_finished_rollup_carries_the_review_only_count() {
    let summary = crate::run::summary::RunSummary::from_report(
        &crate::run::summary::tests::fixture_report(),
        7,
    );
    let v = map(decode_run_finished(&summary), &RunState::default());

    assert_eq!(v["data"]["issues"][0]["number"], 1, "{v}");
    assert_eq!(v["data"]["issues"][0]["review_only"], 2, "{v}");
    // Pin the entry's identity too: `get` on a Null (an index past a truncated
    // array) also returns `None`, so `is_none()` alone proves nothing.
    assert_eq!(v["data"]["issues"][1]["number"], 2, "{v}");
    assert!(
        v["data"]["issues"][1].get("review_only").is_none(),
        "omitted at zero: {v}"
    );
}

/// A legacy emitter carried no rollup: the envelope must still publish the
/// fold-derived `data.issues[]` exactly as it did before #224.
#[test]
fn run_finished_falls_back_to_the_fold_without_a_rollup() {
    let mut state = RunState::new("t", 1);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "one".into(),
    });
    state.apply(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    });
    let v = map(
        RunEvent::RunFinished {
            outcome: "completed".into(),
            issues_done: 1,
            issues_skipped: 0,
            issues_total: 1,
            issues_blocked: 0,
            issues_hitl: 0,
            issues: Value::Null,
            up: 0,
            cr: 0,
            cw: 0,
            out: 0,
            duration_s: 1,
        },
        &state,
    );
    assert_eq!(
        v["data"]["issues"][0],
        json!({"number": 1, "title": "one", "status": "done"})
    );
}
