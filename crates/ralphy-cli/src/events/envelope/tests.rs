use super::*;

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
fn queue_built_has_no_subject_and_lists_order() {
    let v = map(
        RunEvent::QueueBuilt {
            count: 3,
            order: vec![1, 2, 3],
            stop_before: Some(2),
            issues: Value::Null,
            assignee_filter: None,
        },
        &RunState::new("t", 1),
    );
    assert_eq!(v["type"], "dev.ralphy.queue.built");
    assert!(
        v.get("subject").is_none(),
        "queue.built has no subject: {v}"
    );
    assert_eq!(v["data"]["count"], 3);
    assert_eq!(v["data"]["order"], json!([1, 2, 3]));
    assert_eq!(v["data"]["stop_before"], 2);
}

#[test]
fn queue_built_carries_the_enriched_issues_array() {
    // The enriched `queue.built` (ADR-0020) carries the per-issue snapshot
    // verbatim under `data.issues`.
    let issues = json!([
        {"number": 1, "title": "a", "labels": ["q"], "queue_status": "eligible",
         "skip_reason": null, "blocked_by": [], "position": 1},
        {"number": 2, "title": "b", "labels": ["q"], "queue_status": "blocked",
         "skip_reason": null, "blocked_by": [1], "position": null},
    ]);
    let v = map(
        RunEvent::QueueBuilt {
            count: 2,
            order: vec![1, 2],
            stop_before: None,
            issues: issues.clone(),
            assignee_filter: None,
        },
        &RunState::new("t", 1),
    );
    assert_eq!(v["data"]["issues"], issues);
    assert_eq!(v["data"]["count"], 2);
    assert_eq!(v["data"]["order"], json!([1, 2]));
}

#[test]
fn queue_snapshot_data_matches_queue_built_data() {
    // `queue.snapshot` (from `ralphy issues --push`) and the enriched
    // `queue.built` share ONE `data` builder, so their payloads are identical
    // — only the envelope `type` differs (ADR-0020).
    let issues = json!([
        {"number": 1, "queue_status": "eligible", "position": 1},
    ]);
    let built = map(
        RunEvent::QueueBuilt {
            count: 1,
            order: vec![1],
            stop_before: None,
            issues: issues.clone(),
            assignee_filter: None,
        },
        &RunState::new("t", 1),
    );
    let snapshot = queue_snapshot_envelope(
        queue_snapshot_data(&issues, 1, &[1], None, None),
        &ctx(),
        &RunState::new("t", 1),
    );
    assert_eq!(snapshot["type"], "dev.ralphy.queue.snapshot");
    assert!(
        snapshot.get("subject").is_none(),
        "queue.snapshot has no subject: {snapshot}"
    );
    // Byte-identical `data` shape (both merge the same emitter via ctx()).
    assert_eq!(snapshot["data"], built["data"]);
}

#[test]
fn queue_built_and_snapshot_carry_assignee_filter() {
    // ADR-0021 §5: the resolved concrete login rides `data.assignee_filter` on a
    // filtered `queue.built`, JSON `null` on an unfiltered one; and the on-demand
    // `queue.snapshot` carries the identical field with the same semantics.
    let filtered = map(
        RunEvent::QueueBuilt {
            count: 1,
            order: vec![1],
            stop_before: None,
            issues: Value::Null,
            assignee_filter: Some("octocat".into()),
        },
        &RunState::new("t", 1),
    );
    assert_eq!(filtered["data"]["assignee_filter"], "octocat");

    let unfiltered = map(
        RunEvent::QueueBuilt {
            count: 1,
            order: vec![1],
            stop_before: None,
            issues: Value::Null,
            assignee_filter: None,
        },
        &RunState::new("t", 1),
    );
    assert!(
        unfiltered["data"]["assignee_filter"].is_null(),
        "unfiltered queue.built has null assignee_filter: {unfiltered}"
    );

    // The `queue.snapshot` twin shares the field and the whole `data` shape.
    let snapshot = queue_snapshot_envelope(
        queue_snapshot_data(&Value::Null, 1, &[1], None, Some("octocat")),
        &ctx(),
        &RunState::new("t", 1),
    );
    assert_eq!(snapshot["data"]["assignee_filter"], "octocat");
    assert_eq!(snapshot["data"], filtered["data"]);
}

#[test]
fn events_doc_documents_assignee_filter() {
    // The doc catalog must name the new field, so a doc regression fails a test.
    // Path: crates/ralphy-cli/src/events/envelope/ -> ../../../../../ = repo root.
    let doc = include_str!("../../../../../docs/events.md");
    assert!(
        doc.contains("assignee_filter"),
        "docs/events.md must document assignee_filter"
    );
}

#[test]
fn issue_started_carries_number_title_and_subject() {
    let v = map(
        RunEvent::IssueStarted {
            number: 7,
            title: "hello".into(),
        },
        &RunState::new("t", 1),
    );
    assert_eq!(v["type"], "dev.ralphy.issue.started");
    assert_eq!(v["subject"], "issue/7");
    assert_eq!(v["data"]["number"], 7);
    assert_eq!(v["data"]["title"], "hello");
}

#[test]
fn planning_resolves_subject_from_active() {
    let v = map(
        RunEvent::Planning {
            model: Some("opus".into()),
            effort: Some("high".into()),
        },
        &active_state(),
    );
    assert_eq!(v["type"], "dev.ralphy.issue.planning");
    assert_eq!(v["subject"], "issue/7");
    assert_eq!(v["data"]["model"], "opus");
    assert_eq!(v["data"]["effort"], "high");
}

#[test]
fn plan_written_carries_open_steps_usage_and_subject() {
    let v = map(
        RunEvent::PlanWritten {
            number: 7,
            open_steps: 4,
            usage: UsageLite {
                input: 10,
                ..Default::default()
            },
            steps: vec![
                ("do a thing".into(), "open".into()),
                ("do another".into(), "checked".into()),
            ],
        },
        &RunState::new("t", 1),
    );
    assert_eq!(v["type"], "dev.ralphy.plan.written");
    assert_eq!(v["subject"], "issue/7");
    assert_eq!(v["data"]["open_steps"], 4);
    // The parsed checkbox steps ride `data.steps` (#96).
    assert_eq!(
        v["data"]["steps"],
        json!([
            { "text": "do a thing", "status": "open" },
            { "text": "do another", "status": "checked" },
        ])
    );
    assert_eq!(v["data"]["usage"]["up"], 10);
}

#[test]
fn plan_opened_and_closed_carry_raw_plan_md_subject_and_issue_block() {
    let raw = "# Plan for #7\n\n## Steps\n- [ ] do a thing\n";
    let opened = map(
        RunEvent::PlanOpened {
            number: 7,
            plan_md: raw.into(),
        },
        &run_state(),
    );
    assert_eq!(opened["type"], "dev.ralphy.plan.opened");
    assert_eq!(opened["subject"], "issue/7");
    assert_eq!(opened["data"]["plan_md"], raw);
    // The subject-scoped issue block rides along.
    assert_eq!(opened["data"]["issue"]["number"], 7);
    assert_eq!(opened["data"]["issue"]["title"], "hello");

    let closed = map(
        RunEvent::PlanClosed {
            number: 7,
            plan_md: raw.into(),
        },
        &run_state(),
    );
    assert_eq!(closed["type"], "dev.ralphy.plan.closed");
    assert_eq!(closed["subject"], "issue/7");
    assert_eq!(closed["data"]["plan_md"], raw);
    assert_eq!(closed["data"]["issue"]["number"], 7);
}

#[test]
fn executing_resolves_active_number_and_subject() {
    let v = map(
        RunEvent::Executing {
            number: 0,
            budget_min: 45,
            model: "claude-sonnet-4".into(),
            effort: Some("medium".into()),
        },
        &active_state(),
    );
    assert_eq!(v["type"], "dev.ralphy.issue.executing");
    assert_eq!(v["subject"], "issue/7");
    assert_eq!(v["data"]["number"], 7);
    assert_eq!(v["data"]["budget_min"], 45);
    assert_eq!(v["data"]["model"], "claude-sonnet-4");
    assert_eq!(v["data"]["effort"], "medium");
}

#[test]
fn non_green_carries_outcome_and_subject() {
    let v = map(
        RunEvent::NonGreen {
            number: 7,
            outcome: "Stuck".into(),
        },
        &RunState::new("t", 1),
    );
    assert_eq!(v["type"], "dev.ralphy.issue.non_green");
    assert_eq!(v["subject"], "issue/7");
    assert_eq!(v["data"]["outcome"], "Stuck");
}

#[test]
fn needs_split_carries_number_and_subject() {
    let v = map(RunEvent::NeedsSplit { number: 7 }, &RunState::new("t", 1));
    assert_eq!(v["type"], "dev.ralphy.issue.needs_split");
    assert_eq!(v["subject"], "issue/7");
    assert_eq!(v["data"]["number"], 7);
}

#[test]
fn skipped_maps_kind_and_parking_label() {
    // A human-return skip names the parking label.
    let v = map(
        RunEvent::Skipped {
            number: 9,
            kind: SkipKind::HumanReturn,
            label: Some("needs-info".into()),
            blockers: vec![],
        },
        &RunState::new("t", 1),
    );
    assert_eq!(v["type"], "dev.ralphy.issue.skipped");
    assert_eq!(v["subject"], "issue/9");
    assert_eq!(v["data"]["kind"], "human_return");
    assert_eq!(v["data"]["label"], "needs-info");
    // A non-dependency skip carries an empty `blocked_by` list.
    assert_eq!(v["data"]["blocked_by"], json!([]));

    // A blocked-by skip has no parking label, the `blocked_by` kind, and names the
    // still-open blocker(s) in `data.blocked_by`.
    let v = map(
        RunEvent::Skipped {
            number: 4,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![139],
        },
        &RunState::new("t", 1),
    );
    assert_eq!(v["data"]["kind"], "blocked_by");
    assert!(v["data"]["label"].is_null(), "no parking label: {v}");
    assert_eq!(v["data"]["blocked_by"], json!([139]));
}

#[test]
fn human_blocked_lists_blockers() {
    let v = map(
        RunEvent::HumanBlocked {
            number: 16,
            on: vec![30, 18],
        },
        &RunState::new("t", 1),
    );
    assert_eq!(v["type"], "dev.ralphy.issue.human_blocked");
    assert_eq!(v["subject"], "issue/16");
    assert_eq!(v["data"]["on"], json!([30, 18]));
}

#[test]
fn deadline_passed_carries_number_and_subject() {
    let v = map(
        RunEvent::DeadlinePassed { number: 7 },
        &RunState::new("t", 1),
    );
    assert_eq!(v["type"], "dev.ralphy.issue.deadline_passed");
    assert_eq!(v["subject"], "issue/7");
}

#[test]
fn sleep_events_map_without_subject() {
    let v = map(
        RunEvent::SleepStarted {
            reset: "14:30".into(),
            target_epoch: 1_700_000_000,
        },
        &RunState::new("t", 1),
    );
    assert_eq!(v["type"], "dev.ralphy.run.sleep_started");
    assert!(
        v.get("subject").is_none(),
        "sleep_started has no subject: {v}"
    );
    assert_eq!(v["data"]["reset"], "14:30");
    assert_eq!(v["data"]["target_epoch"], 1_700_000_000i64);

    let v = map(RunEvent::SleepEnded, &RunState::new("t", 1));
    assert_eq!(v["type"], "dev.ralphy.run.sleep_ended");
    assert!(
        v.get("subject").is_none(),
        "sleep_ended has no subject: {v}"
    );
}

#[test]
fn api_degraded_events_map_without_subject() {
    let v = map(RunEvent::ApiDegraded, &RunState::new("t", 1));
    assert_eq!(v["type"], "dev.ralphy.run.api_degraded");
    assert!(
        v.get("subject").is_none(),
        "api_degraded has no subject: {v}"
    );

    let v = map(RunEvent::ApiRecovered, &RunState::new("t", 1));
    assert_eq!(v["type"], "dev.ralphy.run.api_recovered");
    assert!(
        v.get("subject").is_none(),
        "api_recovered has no subject: {v}"
    );
}

#[test]
fn knowledge_events_map_counts() {
    let v = map(
        RunEvent::KnowledgeConsolidating { notes: 4 },
        &RunState::new("t", 1),
    );
    assert_eq!(v["type"], "dev.ralphy.knowledge.consolidating");
    assert_eq!(v["data"]["notes"], 4);

    let v = map(
        RunEvent::KnowledgeConsolidated { archived: 3 },
        &RunState::new("t", 1),
    );
    assert_eq!(v["type"], "dev.ralphy.knowledge.consolidated");
    assert_eq!(v["data"]["archived"], 3);
}

#[test]
fn run_started_maps_cli_params_without_subject() {
    let ev = RunEvent::RunStarted {
        repo: "o/r".into(),
        queue_labels: vec!["AFK".into(), "ready".into()],
        agent: "claude".into(),
        plan_agent: "codex".into(),
        branch_mode: "new".into(),
        branch: "origin/main".into(),
        deadline_hours: Some(6.0),
    };
    // The sink folds before mapping, so the `data.agent` block resolves the exec
    // agent from the folded state — fold first here to mirror that. Fold a
    // preceding enriched `queue.built` too, so `data.queue` is seeded.
    let mut state = RunState::new("t", 1);
    state.apply(RunEvent::QueueBuilt {
        count: 2,
        order: vec![1, 2],
        stop_before: None,
        issues: json!([
            {"number": 1, "title": "one"},
            {"number": 2, "title": "two"},
        ]),
        assignee_filter: None,
    });
    state.apply(ev.clone());
    let v = runevent_to_cloudevent(&ev, &ctx(), &state).expect("mapped");
    assert_eq!(v["type"], "dev.ralphy.run.started");
    assert!(
        v.get("subject").is_none(),
        "run.started has no subject: {v}"
    );
    assert_eq!(v["data"]["repo"], "o/r");
    assert_eq!(v["data"]["queue_labels"], json!(["AFK", "ready"]));
    // The exec agent is now the `data.agent` block's `name`, not a scalar.
    assert!(
        v["data"]["agent"].get("name").is_some(),
        "agent is now a block: {v}"
    );
    assert_eq!(v["data"]["agent"]["name"], "claude");
    assert_eq!(v["data"]["plan_agent"], "codex");
    assert_eq!(v["data"]["branch_mode"], "new");
    // The base branch is now under the `base` key (renamed from `branch`, #96).
    assert_eq!(v["data"]["base"], "origin/main");
    assert!(
        v["data"].get("branch").is_none(),
        "branch renamed to base: {v}"
    );
    assert_eq!(v["data"]["deadline_hours"], 6.0);
    // The light queue scope seeded from `queue.built` (#96).
    assert_eq!(
        v["data"]["queue"],
        json!([
            { "number": 1, "title": "one" },
            { "number": 2, "title": "two" },
        ])
    );
}

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
    });
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "one".into(),
    });
    state.apply(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
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
fn notice_maps_level_and_message_without_subject() {
    let v = map(
        RunEvent::Notice {
            level: tracing::Level::WARN,
            message: "heads up".into(),
        },
        &RunState::new("t", 1),
    );
    assert_eq!(v["type"], "dev.ralphy.run.notice");
    assert!(v.get("subject").is_none(), "notice carries no subject: {v}");
    assert_eq!(v["data"]["level"], "warn");
    assert_eq!(v["data"]["message"], "heads up");
}
