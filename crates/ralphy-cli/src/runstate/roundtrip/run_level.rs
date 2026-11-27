//! Round trips of the run-level events: queue, start, finish, stop, sleep,
//! API health, agent state and knowledge consolidation.

use super::*;

/// One emit helper and the event the decoder must read back from it.
type Row = (&'static str, fn(), RunEvent);

fn assert_rows(rows: Vec<Row>) {
    for (name, emit, expected) in rows {
        let ev = one(emit);
        assert_eq!(decode(&ev), Some(expected), "{name}");
    }
}

/// Both shapes, in one test, because the pair IS the encoding: the emitter omits
/// the `number` field entirely for a between-issues stop, and the decoder's
/// blanket `unwrap_or(0)` is what turns that absence into the `0` sentinel. A
/// test of only the `Some` arm would pass against an emitter that wrote
/// `number = 0` explicitly — and that emitter would report a stop "during #0".
#[test]
fn roundtrip_run_stopped() {
    let ev = one(|| ralphy_core::emit::run_stopped(Some(42)));
    assert_eq!(decode(&ev), Some(RunEvent::RunStopped { number: 42 }));

    let ev = one(|| ralphy_core::emit::run_stopped(None));
    assert!(
        ev.fields.number.is_none(),
        "a between-issues stop must emit no `number` field at all"
    );
    assert_eq!(decode(&ev), Some(RunEvent::RunStopped { number: 0 }));
}

/// Sleep, API health, agent state, hooks, skip and knowledge events. `hint` of
/// the usage-limit event rides along for the log but has no decoded home — the
/// pin in `ralphy-core`'s `pins_usage_limit_vocabulary` keeps it emitted. The
/// agent state has `detail` present and absent (an empty detail reaches the bus
/// as `""` and decodes to `None`).
#[test]
fn roundtrip_simple_run_events() {
    let reason = "skipped: run in progress since 2026-07-19 10:00:00, pid 4242";
    let rows: Vec<Row> = vec![
        (
            "usage_limit_waiting",
            || {
                ralphy_core::emit::usage_limit_waiting(
                    "07:30",
                    "2026-07-19T07:25:00Z",
                    1_700_000_000,
                );
            },
            RunEvent::SleepStarted {
                reset: "07:30".into(),
                target_epoch: 1_700_000_000,
            },
        ),
        (
            "reset_reached",
            ralphy_core::emit::reset_reached,
            RunEvent::SleepEnded,
        ),
        (
            "idle_reaped",
            || ralphy_core::emit::idle_reaped(20),
            RunEvent::IdleReaped { idle_minutes: 20 },
        ),
        (
            "agent_state with detail",
            || {
                ralphy_core::emit::agent_state(
                    "waiting",
                    "2026-09-15T10:00:00-03:00",
                    Some("AskUserQuestion: which port?"),
                );
            },
            RunEvent::AgentState {
                state: "waiting".into(),
                since: "2026-09-15T10:00:00-03:00".into(),
                detail: Some("AskUserQuestion: which port?".into()),
            },
        ),
        (
            "agent_state without detail",
            || ralphy_core::emit::agent_state("done", "2026-09-15T10:01:00-03:00", None),
            RunEvent::AgentState {
                state: "done".into(),
                since: "2026-09-15T10:01:00-03:00".into(),
                detail: None,
            },
        ),
        (
            "hooks_unproven_allowed",
            || ralphy_core::emit::hooks_unproven_allowed("x"),
            RunEvent::HooksUnprovenAllowed { cause: "x".into() },
        ),
        (
            "api_degraded",
            ralphy_core::emit::api_degraded,
            RunEvent::ApiDegraded,
        ),
        (
            "api_recovered",
            ralphy_core::emit::api_recovered,
            RunEvent::ApiRecovered,
        ),
        (
            // The `--if-idle` deferral border (#222).
            "run_skipped",
            || {
                ralphy_core::emit::run_skipped(
                    "skipped: run in progress since 2026-07-19 10:00:00, pid 4242",
                );
            },
            RunEvent::RunSkipped {
                reason: reason.into(),
            },
        ),
        (
            "knowledge_consolidating",
            || ralphy_core::emit::knowledge_consolidating(4),
            RunEvent::KnowledgeConsolidating { notes: 4 },
        ),
        (
            "knowledge_consolidated",
            || ralphy_core::emit::knowledge_consolidated(4),
            RunEvent::KnowledgeConsolidated { archived: 4 },
        ),
    ];
    assert_rows(rows);
}

/// The second row holds the three "absent" encodings the helper's scalar
/// signature forces: `0` for "no stop-before in this queue" (issue numbers are
/// ≥ 1) and `""` for "the queue was fetched unfiltered" / "no scope phrase".
/// All must decode back to `None` — a helper that stopped emitting them, or a
/// decoder that stopped folding them, would surface a phantom stop-before at
/// #0 and a scope mark of "". An unparseable snapshot degrades to `Null`.
#[test]
fn roundtrip_queue_built() {
    let rows: Vec<Row> = vec![
        (
            "full",
            || {
                ralphy_core::emit::queue_built(
                    3,
                    "#1 -> #2 -> #3",
                    2,
                    r#"[{"number":1,"queue_status":"eligible"}]"#,
                    "octocat",
                    "labels [AFK]",
                );
            },
            RunEvent::QueueBuilt {
                count: 3,
                order: vec![1, 2, 3],
                stop_before: Some(2),
                issues: serde_json::json!([{"number":1,"queue_status":"eligible"}]),
                assignee_filter: Some("octocat".into()),
                scope: Some("labels [AFK]".into()),
            },
        ),
        (
            "sentinels fold to None",
            || ralphy_core::emit::queue_built(1, "#1", 0, "not json", "", ""),
            RunEvent::QueueBuilt {
                count: 1,
                order: vec![1],
                stop_before: None,
                issues: serde_json::Value::Null,
                assignee_filter: None,
                scope: None,
            },
        ),
    ];
    assert_rows(rows);
}

/// The second row: `0.0` is the "no `--deadline-hours`" sentinel the emitter
/// writes; the decoder must fold it back to `None` rather than report a 0-hour
/// budget, and an empty joined label string is an empty list, not `[""]`.
#[test]
fn roundtrip_run_started() {
    let rows: Vec<Row> = vec![
        (
            "full",
            || {
                ralphy_core::emit::run_started(
                    "o/r",
                    "AFK,ready",
                    "claude",
                    "codex",
                    "new",
                    "origin/main",
                    6.0,
                );
            },
            RunEvent::RunStarted {
                repo: "o/r".into(),
                queue_labels: vec!["AFK".into(), "ready".into()],
                agent: "claude".into(),
                plan_agent: "codex".into(),
                branch_mode: "new".into(),
                branch: "origin/main".into(),
                deadline_hours: Some(6.0),
            },
        ),
        (
            "no deadline, no labels",
            || {
                ralphy_core::emit::run_started(
                    "o/r", "", "claude", "claude", "current", "main", 0.0,
                );
            },
            RunEvent::RunStarted {
                repo: "o/r".into(),
                queue_labels: vec![],
                agent: "claude".into(),
                plan_agent: "claude".into(),
                branch_mode: "current".into(),
                branch: "main".into(),
                deadline_hours: None,
            },
        ),
    ];
    assert_rows(rows);
}

/// The second row is the empty-queue border (#222): the run still emits the
/// full `run finished`, with the `no_work` outcome, every count at 0 and no
/// rollup (the envelope falls back to the fold). A run spans models, so `run
/// finished` deliberately carries no `model`.
#[test]
fn roundtrip_run_finished() {
    let rows: Vec<Row> = vec![
        (
            "completed",
            || {
                ralphy_core::emit::run_finished(
                    "completed",
                    3,
                    1,
                    5,
                    1,
                    0,
                    r#"[{"number":7,"status":"done"}]"#,
                    &usage(),
                    412,
                );
            },
            RunEvent::RunFinished {
                outcome: "completed".into(),
                issues_done: 3,
                issues_skipped: 1,
                issues_total: 5,
                issues_blocked: 1,
                issues_hitl: 0,
                issues: serde_json::json!([{"number": 7, "status": "done"}]),
                up: 11,
                cr: 22,
                cw: 33,
                out: 44,
                duration_s: 412,
            },
        ),
        (
            "no_work",
            || {
                ralphy_core::emit::run_finished(
                    "no_work",
                    0,
                    0,
                    0,
                    0,
                    0,
                    "",
                    &ralphy_core::Usage::default(),
                    0,
                );
            },
            RunEvent::RunFinished {
                outcome: "no_work".into(),
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
            },
        ),
    ];
    for (name, emit, expected) in rows {
        let ev = one(emit);
        assert_eq!(decode(&ev), Some(expected), "{name}");
        assert_eq!(ev.fields.model, None, "{name}: a run carries no model");
    }
}
