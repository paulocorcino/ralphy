//! Pins of the run-boundary and shared-vocabulary events the decoder consumes.

use super::*;

#[test]
fn run_finished_triple_is_pinned() {
    use ralphy_core::{IssueResult, Outcome, QueueReport, ResultStatus, Usage};

    let report = QueueReport {
        branch: "ralphy/run".into(),
        orig_branch: "main".into(),
        worked: vec![IssueResult {
            number: 7,
            outcome: Some(Outcome::Done),
            closed: true,
            blocked_by: Vec::new(),
            human_blockers: Vec::new(),
            status: ResultStatus::Done,
            skip: None,
            review_only: 0,
        }],
        stop: None,
        commits: 1,
        undo_tag: None,
        oneline: Vec::new(),
        run_usage: Usage {
            input: 10,
            cache_read: 20,
            cache_creation: 30,
            output: 40,
            model: None,
        },
        run_usage_by_model: Default::default(),
        invocations: 0,
    };

    let summary = crate::run::summary::RunSummary::from_report(&report, 1);
    let run_usage = report.run_usage.clone();
    let ((), events) = capture_events(|| {
        crate::run::report::emit_run_finished(&summary, &run_usage, std::time::Instant::now());
    });
    let ev = events
        .iter()
        .find(|e| e.message == "run finished")
        .expect("a `run finished` event");

    // The target (the emitting module) is not pinned: the decoder ignores it.
    assert_eq!(ev.level, Level::INFO);
    let f = &ev.fields;
    assert_eq!(f.outcome.as_deref(), Some("completed"));
    assert_eq!(f.issues_done, Some(1));
    assert_eq!(f.issues_skipped, Some(0));
    assert_eq!(f.issues_total, Some(1));
    // The fixture's single issue is green, so both new buckets are 0 — the
    // point of the pin is that the fields are EMITTED, with the run's own
    // rollup beside them.
    assert_eq!(f.issues_blocked, Some(0));
    assert_eq!(f.issues_hitl, Some(0));
    assert_eq!(
        f.issues_json.as_deref(),
        Some(r#"[{"number":7,"status":"done"}]"#)
    );
    assert_eq!(
        (f.up, f.cr, f.cw, f.out),
        (Some(10), Some(20), Some(30), Some(40))
    );
    assert!(f.duration_s.is_some(), "duration_s present");

    // The decoder must read the same triple back into the typed event.
    match crate::runstate::event_to_runevent(&ev.target, &ev.message, f) {
        Some(crate::runstate::RunEvent::RunFinished {
            outcome,
            issues_done,
            issues_skipped,
            issues_total,
            issues_blocked,
            issues_hitl,
            issues,
            up,
            cr,
            cw,
            out,
            duration_s: _,
        }) => {
            assert_eq!(outcome, "completed");
            assert_eq!((issues_done, issues_skipped, issues_total), (1, 0, 1));
            assert_eq!((issues_blocked, issues_hitl), (0, 0));
            let rollup = issues.as_array().expect("the rollup decodes to an array");
            assert_eq!(rollup.len(), 1);
            assert_eq!(rollup[0]["status"], "done");
            assert_eq!((up, cr, cw, out), (10, 20, 30, 40));
        }
        other => panic!("expected RunFinished, got {other:?}"),
    }
}

/// The empty-queue border's own emitter (#222): it does NOT go through
/// `emit_run_finished`/`outcome_of` (no `QueueReport` exists), so its shape is
/// pinned separately — same level, `no_work` and all-zero counts.
#[test]
fn no_work_triple_is_pinned() {
    let ((), events) =
        capture_events(|| crate::run::report::emit_run_finished_no_work(std::time::Instant::now()));
    let ev = events
        .iter()
        .find(|e| e.message == "run finished")
        .expect("a `run finished` event");

    assert_eq!(ev.level, Level::INFO);
    let f = &ev.fields;
    assert_eq!(f.outcome.as_deref(), Some("no_work"));
    assert_eq!(
        (f.issues_done, f.issues_skipped, f.issues_total),
        (Some(0), Some(0), Some(0))
    );
    assert_eq!((f.issues_blocked, f.issues_hitl), (Some(0), Some(0)));
    // No rollup on an empty run: the decoder reads the empty string back as
    // `Value::Null`, the envelope's legacy-fallback signal.
    assert_eq!(f.issues_json.as_deref(), Some(""));
    match crate::runstate::event_to_runevent(&ev.target, &ev.message, f) {
        Some(crate::runstate::RunEvent::RunFinished { issues, .. }) => {
            assert_eq!(issues, serde_json::Value::Null);
        }
        other => panic!("expected RunFinished, got {other:?}"),
    }
}

#[test]
fn shared_vocabulary_constants_are_pinned() {
    use crate::runstate::{event_to_runevent, RunEvent};
    use ralphy_adapter_support::{API_DEGRADED_MSG, API_RECOVERED_MSG, IDLE_REAPED_MSG};

    // The messages' wording is the vocabulary's own; what is pinned is that
    // each decodes to its event.
    assert!(matches!(
        event_to_runevent("t", API_DEGRADED_MSG, &info_fields(|_| {})),
        Some(RunEvent::ApiDegraded)
    ));
    assert!(matches!(
        event_to_runevent("t", API_RECOVERED_MSG, &info_fields(|_| {})),
        Some(RunEvent::ApiRecovered)
    ));
    assert!(matches!(
        event_to_runevent(
            "t",
            IDLE_REAPED_MSG,
            &info_fields(|f| f.idle_minutes = Some(7))
        ),
        Some(RunEvent::IdleReaped { idle_minutes: 7 })
    ));
}
