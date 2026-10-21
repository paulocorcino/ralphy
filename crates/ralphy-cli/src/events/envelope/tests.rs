use super::*;

mod event_data;
mod run_finished;

/// A test context with a stub emitter object.
fn ctx() -> EventCtx {
    EventCtx {
        source: "ralphy/o/r".to_string(),
        runid: "01RUNIDRUNIDRUNIDRUNID".to_string(),
        emitter: json!({ "version": "0.0.0", "pid": 4242 }),
        git: json!({ "repository": "o/r", "branch": "afk/run-t" }),
    }
}

#[test]
fn git_block_is_merged_on_mapped_and_run_envelopes() {
    // The constant-per-run `data.git` block rides every envelope, merged like
    // `emitter` — on a subject-carrying mapped event and on a subject-less
    // run-scoped one (via `run_envelope`).
    let mapped = map(
        RunEvent::IssueStarted {
            number: 7,
            title: "hello".into(),
        },
        &RunState::new("t", 1),
    );
    assert_eq!(mapped["data"]["git"]["repository"], "o/r");
    assert_eq!(mapped["data"]["git"]["branch"], "afk/run-t");

    let run_scoped = run_envelope(
        "dev.ralphy.run.heartbeat",
        &ctx(),
        &RunState::new("t", 1),
        json!({}),
    );
    assert_eq!(run_scoped["data"]["git"]["repository"], "o/r");
    assert_eq!(run_scoped["data"]["git"]["branch"], "afk/run-t");
}

/// A state with `run.started` folded (exec `claude`, plan `codex`) and issue 7
/// active — so agent/issue blocks resolve.
fn run_state() -> RunState {
    let mut s = RunState::new("t", 1);
    s.apply(RunEvent::RunStarted {
        repo: "o/r".into(),
        queue_labels: vec![],
        agent: "claude".into(),
        plan_agent: "codex".into(),
        branch_mode: "new".into(),
        branch: "origin/main".into(),
        deadline_hours: None,
    });
    s.apply(RunEvent::IssueStarted {
        number: 7,
        title: "hello".into(),
    });
    s
}

#[test]
fn agent_block_reflects_phase_and_is_absent_on_run_finished() {
    // Before any phase: name falls back to the run's exec agent, model/effort null.
    let pre = map(
        RunEvent::IssueStarted {
            number: 7,
            title: "hello".into(),
        },
        &run_state(),
    );
    assert_eq!(pre["data"]["agent"]["name"], "claude");
    assert!(
        pre["data"]["agent"]["model"].is_null(),
        "model null pre-phase"
    );
    assert!(
        pre["data"]["agent"]["effort"].is_null(),
        "effort null pre-phase"
    );

    // After a Planning fold: name is the plan agent with its model/effort.
    let mut planning = run_state();
    planning.apply(RunEvent::Planning {
        model: Some("opus".into()),
        effort: Some("high".into()),
    });
    let v = map(RunEvent::NeedsSplit { number: 7 }, &planning);
    assert_eq!(v["data"]["agent"]["name"], "codex");
    assert_eq!(v["data"]["agent"]["model"], "opus");
    assert_eq!(v["data"]["agent"]["effort"], "high");

    // After an Executing fold: name is the exec agent.
    let mut executing = run_state();
    executing.apply(RunEvent::Executing {
        number: 0,
        budget_min: 45,
        model: "claude-sonnet-4".into(),
        effort: Some("medium".into()),
    });
    let v = map(RunEvent::NeedsSplit { number: 7 }, &executing);
    assert_eq!(v["data"]["agent"]["name"], "claude");
    assert_eq!(v["data"]["agent"]["model"], "claude-sonnet-4");

    // `run.finished` carries NO agent block.
    let fin = map(
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
        &run_state(),
    );
    assert!(
        fin["data"].get("agent").is_none(),
        "run.finished has no agent block: {fin}"
    );
}

#[test]
fn agent_name_is_null_before_run_started_folds() {
    // On `queue.built` (precedes `run.started`), the exec agent is unknown → null.
    let v = map(
        RunEvent::QueueBuilt {
            count: 1,
            order: vec![1],
            stop_before: None,
            issues: Value::Null,
            assignee_filter: None,
            scope: None,
        },
        &RunState::new("t", 1),
    );
    assert!(
        v["data"]["agent"]["name"].is_null(),
        "name null before run.started: {v}"
    );
}

#[test]
fn issue_block_present_on_subject_scoped_absent_on_run_scoped() {
    // `issue.started` carries `data.issue.{number,title}`.
    let started = map(
        RunEvent::IssueStarted {
            number: 7,
            title: "hello".into(),
        },
        &run_state(),
    );
    assert_eq!(started["data"]["issue"]["number"], 7);
    assert_eq!(started["data"]["issue"]["title"], "hello");

    // `plan.written` (subject-scoped) carries the issue block too, with the title
    // resolved from the folded state.
    let plan = map(
        RunEvent::PlanWritten {
            number: 7,
            open_steps: 3,
            usage: UsageLite::default(),
            steps: vec![],
        },
        &run_state(),
    );
    assert_eq!(plan["data"]["issue"]["number"], 7);
    assert_eq!(plan["data"]["issue"]["title"], "hello");

    // Run-scoped events carry NO issue block.
    let built = map(
        RunEvent::QueueBuilt {
            count: 1,
            order: vec![1],
            stop_before: None,
            issues: Value::Null,
            assignee_filter: None,
            scope: None,
        },
        &run_state(),
    );
    assert!(
        built["data"].get("issue").is_none(),
        "queue.built has no issue block: {built}"
    );
    let started_run = map(
        RunEvent::RunStarted {
            repo: "o/r".into(),
            queue_labels: vec![],
            agent: "claude".into(),
            plan_agent: "codex".into(),
            branch_mode: "new".into(),
            branch: "origin/main".into(),
            deadline_hours: None,
        },
        &run_state(),
    );
    assert!(
        started_run["data"].get("issue").is_none(),
        "run.started has no issue block: {started_run}"
    );
}

#[test]
fn issue_closed_has_full_envelope_shape() {
    let ev = RunEvent::IssueClosed {
        number: 7,
        tokens: 42,
        invocations: 0,
        usage: UsageLite {
            input: 1,
            cache_read: 2,
            cache_creation: 3,
            output: 4,
            model: Some("claude-sonnet-4".into()),
        },
    };
    let v = runevent_to_cloudevent(&ev, &ctx(), &RunState::new("t", 1)).unwrap();
    assert_eq!(v["specversion"], "1.0");
    assert_eq!(v["type"], "dev.ralphy.issue.closed");
    assert_eq!(v["source"], "ralphy/o/r");
    assert_eq!(v["subject"], "issue/7");
    assert_eq!(v["datacontenttype"], "application/json");
    assert_eq!(v["runid"], "01RUNIDRUNIDRUNIDRUNID");
    // The per-event id and time are present and well-shaped.
    assert!(v["id"].as_str().is_some_and(|s| !s.is_empty()), "id: {v}");
    assert!(
        v["time"].as_str().is_some_and(|s| s.ends_with('Z')),
        "time not UTC: {v}"
    );
    // Data fields + the reserved emitter block merged in.
    assert_eq!(v["data"]["number"], 7);
    assert_eq!(v["data"]["tokens"], 42);
    assert_eq!(v["data"]["usage"]["up"], 1);
    assert_eq!(v["data"]["usage"]["out"], 4);
    assert_eq!(v["data"]["usage"]["model"], "claude-sonnet-4");
    assert_eq!(v["data"]["emitter"]["pid"], 4242);
}

/// A folded state with issue 7 active (so the number-0 adapter events resolve).
fn active_state() -> RunState {
    let mut s = RunState::new("t", 1);
    s.apply(RunEvent::IssueStarted {
        number: 7,
        title: "hello".into(),
    });
    s
}

fn map(ev: RunEvent, state: &RunState) -> Value {
    runevent_to_cloudevent(&ev, &ctx(), state).expect("mapped")
}

#[test]
fn only_extension_attribute_is_runid_git_issue_agent_live_in_data() {
    // A fully-populated, subject-scoped envelope (git + issue + agent + emitter
    // blocks all present) must still carry exactly ONE extension attribute at the
    // top level — `runid` (ADR-0019 §3). The reserved blocks live inside `data`.
    let mut state = run_state();
    state.apply(RunEvent::Planning {
        model: Some("opus".into()),
        effort: Some("high".into()),
    });
    let v = map(
        RunEvent::PlanWritten {
            number: 7,
            open_steps: 3,
            usage: UsageLite::default(),
            steps: vec![("a".into(), "open".into())],
        },
        &state,
    );
    // The reserved blocks are inside `data`, not top-level.
    assert!(v["data"].get("git").is_some(), "git in data: {v}");
    assert!(v["data"].get("issue").is_some(), "issue in data: {v}");
    assert!(v["data"].get("agent").is_some(), "agent in data: {v}");
    assert!(v["data"].get("emitter").is_some(), "emitter in data: {v}");

    // Top-level keys minus the CloudEvents core set must be exactly {runid}.
    let core = [
        "specversion",
        "type",
        "source",
        "subject",
        "id",
        "time",
        "datacontenttype",
        "data",
    ];
    let extensions: Vec<String> = v
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| !core.contains(&k.as_str()))
        .cloned()
        .collect();
    assert_eq!(
        extensions,
        vec!["runid".to_string()],
        "the only extension attribute must be runid: {extensions:?}"
    );
}
#[test]
fn queue_built_envelope_carries_no_scope_key() {
    // `scope` is LOG-ONLY (#222): it folds the console edge notice and must NEVER
    // reach the wire — the `dev.ralphy.queue.built` shape is unchanged.
    let v = map(
        RunEvent::QueueBuilt {
            count: 0,
            order: vec![],
            stop_before: None,
            issues: Value::Null,
            assignee_filter: None,
            scope: Some("labels [AFK]".into()),
        },
        &RunState::new("t", 0),
    );
    assert_eq!(v["type"], "dev.ralphy.queue.built");
    assert!(
        v["data"].get("scope").is_none(),
        "scope is log-only, never on the wire: {v}"
    );
}
/// ADR-0059 §2: the agent's state rides the ACTIVE issue's subject with the
/// three fields verbatim; before any issue is active it is run-scoped.
#[test]
fn agent_state_rides_the_active_issue_subject() {
    let ev = RunEvent::AgentState {
        state: "waiting".into(),
        since: "2026-09-15T10:00:00-03:00".into(),
        detail: Some("AskUserQuestion: which port?".into()),
    };
    let v = map(ev.clone(), &active_state());
    assert_eq!(v["type"], "dev.ralphy.issue.agent_state");
    assert_eq!(v["subject"], "issue/7");
    assert_eq!(v["data"]["state"], "waiting");
    assert_eq!(v["data"]["since"], "2026-09-15T10:00:00-03:00");
    assert_eq!(v["data"]["detail"], "AskUserQuestion: which port?");
    assert!(v["data"].get("interrupted").is_none(), "{v}");
    let v = map(ev, &RunState::new("t", 1));
    assert!(v.get("subject").is_none() || v["subject"].is_null(), "{v}");
}
