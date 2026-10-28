use super::*;

#[test]
fn queue_built_has_no_subject_and_lists_order() {
    let v = map(
        RunEvent::QueueBuilt {
            count: 3,
            order: vec![1, 2, 3],
            stop_before: Some(2),
            issues: Value::Null,
            assignee_filter: None,
            scope: None,
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
            scope: None,
        },
        &RunState::new("t", 1),
    );
    assert_eq!(v["data"]["issues"], issues);
    assert_eq!(v["data"]["count"], 2);
    assert_eq!(v["data"]["order"], json!([1, 2]));
}
/// `queue.snapshot` (from `ralphy issues --push`) and the enriched
/// `queue.built` share ONE `data` builder, so their payloads are identical —
/// only the envelope `type` differs (ADR-0020). ADR-0021 §5: the resolved
/// concrete login rides `data.assignee_filter` on a filtered queue, JSON `null`
/// on an unfiltered one, with the same semantics on both.
#[test]
fn queue_snapshot_data_matches_queue_built_data() {
    // (case, issues, assignee filter)
    let rows = [
        (
            "unfiltered",
            json!([{"number": 1, "queue_status": "eligible", "position": 1}]),
            None,
        ),
        ("filtered", Value::Null, Some("octocat")),
    ];
    for (case, issues, filter) in rows {
        let built = map(
            RunEvent::QueueBuilt {
                count: 1,
                order: vec![1],
                stop_before: None,
                issues: issues.clone(),
                assignee_filter: filter.map(str::to_string),
                scope: None,
            },
            &RunState::new("t", 1),
        );
        let snapshot = queue_snapshot_envelope(
            queue_snapshot_data(&issues, 1, &[1], None, filter),
            &ctx(),
            &RunState::new("t", 1),
        );
        assert_eq!(snapshot["type"], "dev.ralphy.queue.snapshot", "{case}");
        assert!(
            snapshot.get("subject").is_none(),
            "{case}: queue.snapshot has no subject: {snapshot}"
        );
        assert_eq!(built["data"]["assignee_filter"], json!(filter), "{case}");
        // Byte-identical `data` shape (both merge the same emitter via ctx()).
        assert_eq!(snapshot["data"], built["data"], "{case}");
    }
}
/// Each run event maps to its CloudEvent `type`, carries `subject:
/// issue/<n>` only when it is about one issue (a run-scoped event carries
/// none), and puts its fields under `data`.
#[test]
fn each_run_event_maps_to_its_type_subject_and_data() {
    let reason = "skipped: run in progress since 2026-07-19 10:00:00, pid 4242";
    // (event, run size, type, subject, data fields)
    type Row<'a> = (
        RunEvent,
        usize,
        &'a str,
        Option<&'a str>,
        Vec<(&'a str, Value)>,
    );
    let rows: Vec<Row> = vec![
        (
            RunEvent::IssueStarted {
                number: 7,
                title: "hello".into(),
            },
            1,
            "dev.ralphy.issue.started",
            Some("issue/7"),
            vec![("number", json!(7)), ("title", json!("hello"))],
        ),
        (
            RunEvent::NonGreen {
                number: 7,
                outcome: "Stuck".into(),
            },
            1,
            "dev.ralphy.issue.non_green",
            Some("issue/7"),
            vec![("outcome", json!("Stuck"))],
        ),
        (
            RunEvent::NeedsSplit { number: 7 },
            1,
            "dev.ralphy.issue.needs_split",
            Some("issue/7"),
            vec![("number", json!(7))],
        ),
        (
            RunEvent::HumanBlocked {
                number: 16,
                on: vec![30, 18],
            },
            1,
            "dev.ralphy.issue.human_blocked",
            Some("issue/16"),
            vec![("on", json!([30, 18]))],
        ),
        (
            RunEvent::DeadlinePassed { number: 7 },
            1,
            "dev.ralphy.issue.deadline_passed",
            Some("issue/7"),
            vec![],
        ),
        (
            RunEvent::SleepStarted {
                reset: "14:30".into(),
                target_epoch: 1_700_000_000,
            },
            1,
            "dev.ralphy.run.sleep_started",
            None,
            vec![
                ("reset", json!("14:30")),
                ("target_epoch", json!(1_700_000_000i64)),
            ],
        ),
        (
            RunEvent::SleepEnded,
            1,
            "dev.ralphy.run.sleep_ended",
            None,
            vec![],
        ),
        (
            RunEvent::ApiDegraded,
            1,
            "dev.ralphy.run.api_degraded",
            None,
            vec![],
        ),
        (
            RunEvent::ApiRecovered,
            1,
            "dev.ralphy.run.api_recovered",
            None,
            vec![],
        ),
        (
            RunEvent::KnowledgeConsolidating { notes: 4 },
            1,
            "dev.ralphy.knowledge.consolidating",
            None,
            vec![("notes", json!(4))],
        ),
        (
            RunEvent::KnowledgeConsolidated { archived: 3 },
            1,
            "dev.ralphy.knowledge.consolidated",
            None,
            vec![("archived", json!(3))],
        ),
        (
            RunEvent::Notice {
                level: tracing::Level::WARN,
                message: "heads up".into(),
            },
            1,
            "dev.ralphy.run.notice",
            None,
            vec![("level", json!("warn")), ("message", json!("heads up"))],
        ),
        (
            RunEvent::RunSkipped {
                reason: reason.into(),
            },
            0,
            "dev.ralphy.run.skipped",
            None,
            vec![("reason", json!(reason))],
        ),
    ];
    for (event, total, kind, subject, data) in rows {
        let v = map(event, &RunState::new("t", total));
        assert_eq!(v["type"], kind);
        assert_eq!(
            v.get("subject").and_then(Value::as_str),
            subject,
            "{kind}: {v}"
        );
        for (key, want) in data {
            assert_eq!(v["data"][key], want, "{kind}: data.{key}");
        }
    }
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
        scope: None,
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
fn hooks_unproven_allowed_maps_to_its_cloudevent() {
    let v = map(
        RunEvent::HooksUnprovenAllowed {
            cause: "no SessionStart hook event came in 30 s".into(),
        },
        &RunState::new("t", 1),
    );
    assert_eq!(v["type"], "dev.ralphy.run.hooks_unproven_allowed");
    assert!(v.get("subject").is_none(), "a run-level event: {v}");
    assert_eq!(
        v["data"]["cause"],
        "no SessionStart hook event came in 30 s"
    );
}
