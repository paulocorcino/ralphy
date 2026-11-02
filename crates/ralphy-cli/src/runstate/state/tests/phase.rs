//! Tests of the phase context and the per-issue render facts.

use super::*;

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
