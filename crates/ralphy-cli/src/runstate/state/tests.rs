use super::*;
use crate::runstate::event::event_to_runevent;
use crate::runstate::{EventFields, UsageLite};
use tracing::Level;

#[test]
fn api_degraded_folds_the_degraded_flag() {
    let mut s = RunState::new("t", 1);
    assert!(!s.degraded);
    s.apply(RunEvent::ApiDegraded);
    assert!(s.degraded);
    s.apply(RunEvent::ApiRecovered);
    assert!(!s.degraded);
}

#[test]
fn full_lifecycle_yields_expected_statuses_and_summary() {
    let events = vec![
        RunEvent::QueueBuilt {
            count: 2,
            order: vec![1, 2],
            stop_before: None,
            issues: serde_json::Value::Null,
            assignee_filter: None,
            scope: None,
        },
        RunEvent::IssueStarted {
            number: 1,
            title: "one".into(),
        },
        RunEvent::PlanWritten {
            number: 1,
            open_steps: 3,
            usage: UsageLite::default(),
            steps: vec![],
        },
        // The execution event carries no number; it must land on the active issue.
        RunEvent::Executing {
            number: 0,
            budget_min: 45,
            model: String::new(),
            effort: None,
        },
        RunEvent::IssueClosed {
            number: 1,
            tokens: 0,
            invocations: 0,
            usage: UsageLite::default(),
        },
        RunEvent::IssueStarted {
            number: 2,
            title: "two".into(),
        },
        RunEvent::PlanWritten {
            number: 2,
            open_steps: 2,
            usage: UsageLite::default(),
            steps: vec![],
        },
        RunEvent::Executing {
            number: 0,
            budget_min: 45,
            model: String::new(),
            effort: None,
        },
        RunEvent::NonGreen {
            number: 2,
            outcome: "Stuck".into(),
        },
    ];
    let state = fold("title", 2, events);
    assert_eq!(state.total, 2);
    assert_eq!(state.issues.len(), 2);
    assert_eq!(state.issues[0].status, IssueStatus::Done);
    assert_eq!(state.issues[0].title, "one");
    assert_eq!(state.issues[1].status, IssueStatus::NonGreen);
    let summary = state.final_summary.as_deref().unwrap();
    assert!(summary.contains("#2"), "summary: {summary}");
    assert!(summary.contains("Stuck"), "summary: {summary}");
}

#[test]
fn plan_written_with_zero_steps_is_infeasible() {
    let mut state = RunState::new("t", 1);
    state.apply(RunEvent::IssueStarted {
        number: 5,
        title: "x".into(),
    });
    state.apply(RunEvent::PlanWritten {
        number: 5,
        open_steps: 0,
        usage: UsageLite::default(),
        steps: vec![],
    });
    assert_eq!(state.issues[0].status, IssueStatus::Infeasible);
}

#[test]
fn needs_split_upgrades_infeasible_and_decodes_from_stable_message() {
    // The runner emits "plan written" (0 steps) then "bundle plan — needs
    // split"; the fold must land on NeedsSplit, not stay Infeasible.
    let mut state = RunState::new("t", 1);
    state.apply(RunEvent::IssueStarted {
        number: 3,
        title: "W1 bundle".into(),
    });
    state.apply(RunEvent::PlanWritten {
        number: 3,
        open_steps: 0,
        usage: UsageLite::default(),
        steps: vec![],
    });
    assert_eq!(state.issues[0].status, IssueStatus::Infeasible);
    state.apply(RunEvent::NeedsSplit { number: 3 });
    assert_eq!(state.issues[0].status, IssueStatus::NeedsSplit);
    assert!(state.issues[0].status.is_terminal());
    assert_eq!(state.counts().needs_split, 1);
    assert_eq!(state.counts().infeasible, 0);

    // Decoder: the stable runner message maps to the typed event.
    assert_eq!(
        event_to_runevent(
            "ralphy_core::runner",
            "bundle plan — needs split",
            &EventFields {
                message: "bundle plan — needs split".into(),
                number: Some(3),
                ..Default::default()
            }
        ),
        Some(RunEvent::NeedsSplit { number: 3 })
    );
}

#[test]
fn skipped_event_sets_skipped_status() {
    let mut state = RunState::new("t", 1);
    state.apply(RunEvent::Skipped {
        number: 9,
        kind: SkipKind::BlockedBy,
        label: None,
        blockers: vec![7],
    });
    assert_eq!(state.issues[0].status, IssueStatus::Skipped);
    // The open-blocker list is retained on the entry for the card / rollup.
    assert_eq!(state.issues[0].blocked_by, vec![7]);
}

#[test]
fn non_green_blocked_outcome_maps_to_blocked() {
    let mut state = RunState::new("t", 1);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    state.apply(RunEvent::NonGreen {
        number: 1,
        outcome: "Blocked".into(),
    });
    assert_eq!(state.issues[0].status, IssueStatus::Blocked);
}

#[test]
fn deadline_event_sets_terminal_summary() {
    let mut state = RunState::new("t", 3);
    state.apply(RunEvent::DeadlinePassed { number: 7 });
    assert!(state.final_summary.as_deref().unwrap().contains("#7"));
}

#[test]
fn zero_numbered_event_without_active_is_ignored() {
    // An `Executing` (number 0) whose `IssueStarted` was dropped under
    // back-pressure must not materialize a phantom issue `#0`.
    let mut state = RunState::new("t", 1);
    state.apply(RunEvent::Executing {
        number: 0,
        budget_min: 45,
        model: String::new(),
        effort: None,
    });
    assert!(state.issues.is_empty());
}

#[test]
fn sleep_started_sets_state_and_sleep_ended_clears_it() {
    let mut state = RunState::new("t", 1);
    assert!(state.sleep.is_none());
    state.apply(RunEvent::SleepStarted {
        reset: "14:30".into(),
        target_epoch: 1_700_000_000,
    });
    let sleep = state.sleep.as_ref().expect("sleep set on start");
    assert_eq!(sleep.reset, "14:30");
    assert_eq!(sleep.target_epoch, 1_700_000_000);
    state.apply(RunEvent::SleepEnded);
    assert!(state.sleep.is_none(), "resume clears the sleep");
}

#[test]
fn counts_tally_each_status() {
    let mut state = RunState::new("t", 4);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    state.apply(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    });
    state.apply(RunEvent::Skipped {
        number: 2,
        kind: SkipKind::BlockedBy,
        label: None,
        blockers: vec![],
    });
    state.apply(RunEvent::IssueStarted {
        number: 3,
        title: "c".into(),
    });
    let c = state.counts();
    assert_eq!(c.done, 1);
    assert_eq!(c.skipped, 1);
    assert_eq!(c.planning, 1);
    assert_eq!(state.active_issue().map(|e| e.number), Some(3));
    assert_eq!(state.most_recent_finished().map(|e| e.number), Some(2));
}

#[test]
fn human_blocked_is_its_own_status_and_bucket() {
    // A HumanBlocked event folds to the Hitl status (not generic Skipped) and
    // tallies its own bucket — so the card and counts surface "waiting on
    // human" distinctly (ADR-0014).
    let mut state = RunState::new("t", 2);
    state.apply(RunEvent::HumanBlocked {
        number: 5,
        on: vec![30],
    });
    let entry = state.issues.iter().find(|e| e.number == 5).unwrap();
    assert_eq!(entry.status, IssueStatus::Hitl);
    let c = state.counts();
    assert_eq!(c.hitl, 1);
    assert_eq!(c.skipped, 0, "a human gate is not a generic skip");
}

#[test]
fn apply_knowledge_consolidation_sets_then_clears_live_and_records_count() {
    let mut state = RunState::new("t", 1);
    state.apply(RunEvent::KnowledgeConsolidating { notes: 4 });
    assert_eq!(state.consolidating, Some(4));
    assert_eq!(state.consolidated, None);
    // Completion clears the live flag and records the archived tally.
    state.apply(RunEvent::KnowledgeConsolidated { archived: 4 });
    assert_eq!(state.consolidating, None);
    assert_eq!(state.consolidated, Some(4));
}

#[test]
fn apply_run_boundary_events_leave_the_card_fold_unchanged() {
    // `run.started` now seeds the agent-context fields (for the `data.agent`
    // block), but must not perturb the card fold — issues, counts, active, or
    // any status. `run.finished` stays a full no-op.
    let mut before = RunState::new("t", 1);
    before.apply(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    let mut after = before.clone();
    after.apply(RunEvent::RunStarted {
        repo: "o/r".into(),
        queue_labels: vec![],
        agent: "claude".into(),
        plan_agent: "codex".into(),
        branch_mode: "new".into(),
        branch: "origin/main".into(),
        deadline_hours: None,
    });
    after.apply(RunEvent::RunFinished {
        outcome: "completed".into(),
        issues_done: 1,
        issues_skipped: 0,
        issues_total: 1,
        issues_blocked: 0,
        issues_hitl: 0,
        issues: serde_json::Value::Null,
        up: 0,
        cr: 0,
        cw: 0,
        out: 0,
        duration_s: 1,
    });
    // The card-visible fold is byte-unchanged.
    assert_eq!(before.issues, after.issues);
    assert_eq!(before.counts(), after.counts());
    assert_eq!(before.active, after.active);
    // But the agent identities are now seeded from `run.started`.
    assert_eq!(after.exec_agent, "claude");
    assert_eq!(after.plan_agent, "codex");
}

#[test]
fn apply_notice_is_noop_on_runstate() {
    let mut before = RunState::new("t", 1);
    before.apply(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    let mut after = before.clone();
    after.apply(RunEvent::Notice {
        level: Level::WARN,
        message: "some warning".into(),
    });
    assert_eq!(before, after);
}

#[test]
fn apply_skipped_with_all_kinds_sets_skipped_status() {
    let mut state = RunState::new("t", 3);
    state.apply(RunEvent::Skipped {
        number: 1,
        kind: SkipKind::BlockedBy,
        label: None,
        blockers: vec![],
    });
    state.apply(RunEvent::Skipped {
        number: 2,
        kind: SkipKind::StopBefore,
        label: None,
        blockers: vec![],
    });
    state.apply(RunEvent::Skipped {
        number: 3,
        kind: SkipKind::HumanReturn,
        label: Some("wontfix".into()),
        blockers: vec![],
    });
    assert_eq!(state.issues[0].status, IssueStatus::Skipped);
    assert_eq!(state.issues[1].status, IssueStatus::Skipped);
    assert_eq!(state.issues[2].status, IssueStatus::Skipped);
}

#[test]
fn queue_built_seeds_queue_ref_not_issues() {
    // The enriched snapshot seeds `state.queue` ({number,title}) but leaves
    // `state.issues` empty — so the Telegram card renders nothing until an issue
    // actually starts.
    let mut state = RunState::new("t", 2);
    state.apply(RunEvent::QueueBuilt {
        count: 2,
        order: vec![1, 2],
        stop_before: None,
        issues: serde_json::json!([
            {"number": 1, "title": "one"},
            {"number": 2, "title": "two"},
        ]),
        assignee_filter: None,
        scope: None,
    });
    assert_eq!(
        state.queue,
        vec![
            QueueRef {
                number: 1,
                title: "one".into()
            },
            QueueRef {
                number: 2,
                title: "two".into()
            },
        ]
    );
    assert!(state.issues.is_empty(), "queue.built must not touch issues");
    // A legacy `Null` snapshot leaves the queue empty rather than panicking.
    let mut legacy = RunState::new("t", 1);
    legacy.apply(RunEvent::QueueBuilt {
        count: 1,
        order: vec![1],
        stop_before: None,
        issues: serde_json::Value::Null,
        assignee_filter: None,
        scope: None,
    });
    assert!(legacy.queue.is_empty());
}

#[test]
fn apply_threads_phase_agent_context() {
    // `run.started` seeds the plan/exec identities; a `Planning` fold flips the
    // current agent to the plan agent (with its model/effort), an `Executing`
    // fold to the exec agent — the source of the `data.agent` block (#96).
    let mut state = RunState::new("t", 1);
    assert_eq!(state.cur_agent, None);
    assert_eq!(state.cur_model, None);
    state.apply(RunEvent::RunStarted {
        repo: "o/r".into(),
        queue_labels: vec![],
        agent: "claude".into(),
        plan_agent: "codex".into(),
        branch_mode: "new".into(),
        branch: "origin/main".into(),
        deadline_hours: None,
    });
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    state.apply(RunEvent::Planning {
        model: Some("opus".into()),
        effort: Some("high".into()),
    });
    assert_eq!(state.cur_agent.as_deref(), Some("codex"));
    assert_eq!(state.cur_model.as_deref(), Some("opus"));
    assert_eq!(state.cur_effort.as_deref(), Some("high"));
    state.apply(RunEvent::Executing {
        number: 0,
        budget_min: 45,
        model: "claude-sonnet-4".into(),
        effort: Some("medium".into()),
    });
    assert_eq!(state.cur_agent.as_deref(), Some("claude"));
    assert_eq!(state.cur_model.as_deref(), Some("claude-sonnet-4"));
    assert_eq!(state.cur_effort.as_deref(), Some("medium"));
    // An empty exec model degrades to `None` rather than an empty-string label.
    state.apply(RunEvent::Executing {
        number: 0,
        budget_min: 45,
        model: String::new(),
        effort: None,
    });
    assert_eq!(state.cur_model, None);
}

#[test]
fn issue_started_supersedes_a_non_terminal_prior_issue() {
    // The dry-run bug: a plan-only pass left the prior issue perenially
    // "planning". The next `issue started` is the fold's supersede edge.
    let mut state = RunState::new("t", 2);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "one".into(),
    });
    state.apply(RunEvent::PlanWritten {
        number: 1,
        open_steps: 3,
        usage: UsageLite::default(),
        steps: vec![],
    });
    state.apply(RunEvent::IssueStarted {
        number: 2,
        title: "two".into(),
    });
    assert_eq!(state.issues[0].status, IssueStatus::Planned);
    let c = state.counts();
    assert_eq!(c.planned, 1);
    assert_eq!(c.planning, 1, "only the new issue is still planning");
}

#[test]
fn per_issue_render_facts_fold_with_presenter_semantics() {
    // The presenter's per-field rules, now in the fold: `Planning` writes only
    // what is `Some`, `Executing` overwrites the model unconditionally (empty
    // string included), and `IssueStarted` resets the facts per issue.
    let mut state = RunState::new("t", 2);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "one".into(),
    });
    state.apply(RunEvent::Planning {
        model: Some("opus".into()),
        effort: None,
    });
    state.apply(RunEvent::Executing {
        number: 0,
        budget_min: 45,
        model: String::new(),
        effort: Some("medium".into()),
    });
    assert_eq!(state.issues[0].model, Some(String::new()));
    assert_eq!(state.issues[0].effort.as_deref(), Some("medium"));
    assert_eq!(state.issues[0].budget_min, Some(45));
    state.apply(RunEvent::IssueStarted {
        number: 2,
        title: "two".into(),
    });
    assert_eq!(state.issues[1].model, None);
    assert_eq!(state.issues[1].budget_min, None);
}

#[test]
fn queue_built_seeds_the_working_order_and_stop_before_cut() {
    let mut state = RunState::new("t", 3);
    state.apply(RunEvent::QueueBuilt {
        count: 3,
        order: vec![1, 2, 3],
        stop_before: Some(3),
        issues: serde_json::Value::Null,
        assignee_filter: None,
        scope: None,
    });
    assert_eq!(state.order, vec![1, 2, 3]);
    assert_eq!(state.stop_before, Some(3));
}

#[test]
fn planned_status_wire_is_additive() {
    assert_eq!(IssueStatus::Planned.status_wire(), Some("planned"));
    assert!(IssueStatus::Planned.is_terminal());
}

#[test]
fn run_phase_reports_sleeping_over_executing() {
    let mut state = RunState::new("t", 1);
    assert_eq!(state.run_phase(), "starting");
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    assert_eq!(state.run_phase(), "planning");
    state.apply(RunEvent::Executing {
        number: 1,
        budget_min: 45,
        model: "opus".into(),
        effort: None,
    });
    assert_eq!(state.run_phase(), "executing");
    state.apply(RunEvent::SleepStarted {
        reset: "14:30".into(),
        target_epoch: 1_700_000_000,
    });
    assert_eq!(
        state.run_phase(),
        "sleeping",
        "a usage-limit pause outranks the executing issue"
    );
    state.apply(RunEvent::SleepEnded);
    state.apply(RunEvent::KnowledgeConsolidating { notes: 2 });
    assert_eq!(state.run_phase(), "consolidating");
}

#[test]
fn apply_executing_with_model_sets_executing_status() {
    let mut state = RunState::new("t", 1);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    state.apply(RunEvent::Executing {
        number: 1,
        budget_min: 45,
        model: "claude-opus-4".into(),
        effort: None,
    });
    assert_eq!(state.issues[0].status, IssueStatus::Executing);
}
