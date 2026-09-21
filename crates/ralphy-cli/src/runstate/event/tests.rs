use super::*;

#[test]
fn parse_u64_list_reads_debug_vec_and_tolerates_empty() {
    assert_eq!(parse_u64_list(Some("[30]")), vec![30]);
    assert_eq!(parse_u64_list(Some("[30, 18]")), vec![30, 18]);
    assert!(parse_u64_list(Some("[]")).is_empty());
    assert!(parse_u64_list(None).is_empty());
}

fn decode(fields: EventFields) -> Option<RunEvent> {
    event_to_runevent("ralphy_core::runner", &fields.message.clone(), &fields)
}

#[test]
fn decoder_maps_the_idle_reap_from_either_execution_path() {
    // The normalization this pins (docs/adr/0038): the PTY driver and the
    // headless driver measure progress differently, but both emit the SAME
    // shared constant, so one decoder arm serves both and the operator gets
    // one event shape regardless of which child shape ran.
    assert_eq!(
        decode(EventFields {
            message: ralphy_adapter_support::IDLE_REAPED_MSG.into(),
            idle_minutes: Some(20),
            ..Default::default()
        }),
        Some(RunEvent::IdleReaped { idle_minutes: 20 })
    );
    assert_eq!(
        decode(EventFields {
            message: ralphy_adapter_support::IDLE_REAPED_MSG.into(),
            idle_minutes: Some(45),
            ..Default::default()
        }),
        Some(RunEvent::IdleReaped { idle_minutes: 45 })
    );
}

#[test]
fn the_idle_reap_is_emitted_below_warn_so_it_stays_a_first_class_event() {
    // The decoder short-circuits WARN/ERROR into a generic `Notice`. A reap
    // logged at WARN would therefore lose its identity — no `IdleReaped`, no
    // CloudEvent, no dedicated Telegram push. This pins the level contract the
    // two emitters must honor.
    assert_eq!(
        decode(EventFields {
            message: ralphy_adapter_support::IDLE_REAPED_MSG.into(),
            idle_minutes: Some(20),
            level: Level::WARN,
            ..Default::default()
        }),
        Some(RunEvent::Notice {
            level: Level::WARN,
            message: ralphy_adapter_support::IDLE_REAPED_MSG.into(),
        }),
        "if this ever changes, the emitters may log the reap at WARN again"
    );
}

#[test]
fn decoder_maps_queue_built_assignee_filter() {
    // ADR-0021 §5: a `queue built` carrying the resolved login decodes it onto
    // `QueueBuilt.assignee_filter`; a field-absent `queue built` decodes to `None`.
    assert_eq!(
        decode(EventFields {
            message: "queue built".into(),
            count: Some(1),
            order: Some("#1".into()),
            assignee_filter: Some("octocat".into()),
            ..Default::default()
        }),
        Some(RunEvent::QueueBuilt {
            count: 1,
            order: vec![1],
            stop_before: None,
            issues: serde_json::Value::Null,
            assignee_filter: Some("octocat".into()),
            scope: None,
        })
    );
    assert_eq!(
        decode(EventFields {
            message: "queue built".into(),
            count: Some(1),
            order: Some("#1".into()),
            ..Default::default()
        }),
        Some(RunEvent::QueueBuilt {
            count: 1,
            order: vec![1],
            stop_before: None,
            issues: serde_json::Value::Null,
            assignee_filter: None,
            scope: None,
        })
    );
}

#[test]
fn decoder_maps_each_consumed_info_shape() {
    assert_eq!(
        decode(EventFields {
            message: "queue built".into(),
            count: Some(3),
            order: Some("#1 -> #2 -> #3".into()),
            stop_before: Some(2),
            issues_json: Some(r#"[{"number":1,"queue_status":"eligible"}]"#.into()),
            ..Default::default()
        }),
        Some(RunEvent::QueueBuilt {
            count: 3,
            order: vec![1, 2, 3],
            stop_before: Some(2),
            issues: serde_json::json!([{"number":1,"queue_status":"eligible"}]),
            assignee_filter: None,
            scope: None,
        })
    );
    // A legacy `queue built` with no snapshot decodes with `issues: Null`.
    assert_eq!(
        decode(EventFields {
            message: "queue built".into(),
            count: Some(1),
            order: Some("#1".into()),
            ..Default::default()
        }),
        Some(RunEvent::QueueBuilt {
            count: 1,
            order: vec![1],
            stop_before: None,
            issues: serde_json::Value::Null,
            assignee_filter: None,
            scope: None,
        })
    );
    assert_eq!(
        decode(EventFields {
            message: "issue started".into(),
            number: Some(7),
            title: Some("hello".into()),
            ..Default::default()
        }),
        Some(RunEvent::IssueStarted {
            number: 7,
            title: "hello".into()
        })
    );
    assert_eq!(
        decode(EventFields {
            message: "plan written".into(),
            number: Some(7),
            open_steps: Some(0),
            up: Some(12_400),
            cr: Some(184_000),
            cw: Some(8_100),
            out: Some(3_200),
            model: Some("claude-opus-4".into()),
            steps_json: Some(
                r#"[{"text":"a","status":"open"},{"text":"b","status":"checked"}]"#.into(),
            ),
            ..Default::default()
        }),
        Some(RunEvent::PlanWritten {
            number: 7,
            open_steps: 0,
            usage: UsageLite {
                input: 12_400,
                cache_read: 184_000,
                cache_creation: 8_100,
                output: 3_200,
                model: Some("claude-opus-4".into()),
            },
            steps: vec![("a".into(), "open".into()), ("b".into(), "checked".into()),],
        })
    );
    // The adapter's planning event seeds the planning spinner's model/effort.
    assert_eq!(
        decode(EventFields {
            message: ralphy_core::emit::PLANNING_MSG.into(),
            model: Some("opus".into()),
            effort: Some("high".into()),
            ..Default::default()
        }),
        Some(RunEvent::Planning {
            model: Some("opus".into()),
            effort: Some("high".into()),
        })
    );
    assert_eq!(
        decode(EventFields {
            message: ralphy_core::emit::EXECUTING_MSG.into(),
            budget_min: Some(45),
            model: Some("claude-sonnet-4".into()),
            effort: Some("medium".into()),
            ..Default::default()
        }),
        Some(RunEvent::Executing {
            number: 0,
            budget_min: 45,
            model: "claude-sonnet-4".into(),
            effort: Some("medium".into()),
        })
    );
    assert_eq!(
        decode(EventFields {
            message: ralphy_core::emit::EXECUTING_MSG.into(),
            budget_min: Some(30),
            ..Default::default()
        }),
        Some(RunEvent::Executing {
            number: 0,
            budget_min: 30,
            model: String::new(),
            effort: None,
        })
    );
    assert_eq!(
        decode(EventFields {
            message: "green — issue closed".into(),
            number: Some(7),
            tokens: Some(1_200_000),
            invocations: Some(3),
            up: Some(41_200),
            cr: Some(902_000),
            cw: Some(22_000),
            out: Some(18_400),
            model: Some("claude-sonnet-4".into()),
            ..Default::default()
        }),
        Some(RunEvent::IssueClosed {
            number: 7,
            tokens: 1_200_000,
            invocations: 3,
            usage: UsageLite {
                input: 41_200,
                cache_read: 902_000,
                cache_creation: 22_000,
                output: 18_400,
                model: Some("claude-sonnet-4".into()),
            },
        })
    );
    assert_eq!(
        decode(EventFields {
            message: "non-green — stopping run".into(),
            number: Some(7),
            outcome: Some("Stuck".into()),
            ..Default::default()
        }),
        Some(RunEvent::NonGreen {
            number: 7,
            outcome: "Stuck".into()
        })
    );
    assert_eq!(
        decode(EventFields {
            message: "blocked by open issue(s) — skipping".into(),
            number: Some(7),
            ..Default::default()
        }),
        Some(RunEvent::Skipped {
            number: 7,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![],
        })
    );
    assert_eq!(
        decode(EventFields {
            message: "stop-before label — halting run before this issue".into(),
            number: Some(8),
            ..Default::default()
        }),
        Some(RunEvent::Skipped {
            number: 8,
            kind: SkipKind::StopBefore,
            label: None,
            blockers: vec![],
        })
    );
    assert_eq!(
        decode(EventFields {
            message: "human-return label — skipping issue".into(),
            number: Some(9),
            label: Some("needs-info".into()),
            ..Default::default()
        }),
        Some(RunEvent::Skipped {
            number: 9,
            kind: SkipKind::HumanReturn,
            label: Some("needs-info".into()),
            blockers: vec![],
        })
    );
    assert_eq!(
        decode(EventFields {
            message: "blocked — waiting on human".into(),
            number: Some(16),
            human_blockers: Some("[30]".into()),
            ..Default::default()
        }),
        Some(RunEvent::HumanBlocked {
            number: 16,
            on: vec![30]
        })
    );
}

#[test]
fn decoder_maps_kimi_planning_and_executing() {
    // The kimi adapter's tracing strings must flip the active issue's phase
    // exactly like the other adapters; without them the live line, the
    // Telegram card, and the heartbeat phase all stay stuck on "planning".
    assert_eq!(
        decode(EventFields {
            message: ralphy_core::emit::PLANNING_MSG.into(),
            model: Some("kimi-code".into()),
            ..Default::default()
        }),
        Some(RunEvent::Planning {
            model: Some("kimi-code".into()),
            effort: None,
        })
    );
    assert_eq!(
        decode(EventFields {
            message: ralphy_core::emit::EXECUTING_MSG.into(),
            budget_min: Some(30),
            model: Some("kimi-code".into()),
            ..Default::default()
        }),
        Some(RunEvent::Executing {
            number: 0,
            budget_min: 30,
            model: "kimi-code".into(),
            effort: None,
        })
    );
}

#[test]
fn decoder_reads_blocked_by_blockers_and_tolerates_absence() {
    // A dependency skip carrying the open-blocker list decodes it onto
    // `Skipped.blockers`; an absent `blockers` field decodes to an empty vec.
    assert_eq!(
        decode(EventFields {
            message: "blocked by open issue(s) — skipping".into(),
            number: Some(140),
            blockers: Some("[139]".into()),
            ..Default::default()
        }),
        Some(RunEvent::Skipped {
            number: 140,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![139],
        })
    );
    assert_eq!(
        decode(EventFields {
            message: "blocked by open issue(s) — skipping".into(),
            number: Some(140),
            ..Default::default()
        }),
        Some(RunEvent::Skipped {
            number: 140,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![],
        })
    );
}

#[test]
fn decoder_maps_api_degraded_events() {
    assert_eq!(
        decode(EventFields {
            message: ralphy_adapter_support::API_DEGRADED_MSG.into(),
            ..Default::default()
        }),
        Some(RunEvent::ApiDegraded)
    );
    assert_eq!(
        decode(EventFields {
            message: ralphy_adapter_support::API_RECOVERED_MSG.into(),
            ..Default::default()
        }),
        Some(RunEvent::ApiRecovered)
    );
}

#[test]
fn decoder_maps_the_api_degraded_from_either_execution_path() {
    // The normalization this pins (issue #217): the PTY driver (#149) and the
    // headless driver both emit the SAME shared constant, so one decoder arm
    // serves both and the operator gets one event shape regardless of which
    // child shape ran (mold of `decoder_maps_the_idle_reap_from_either_execution_path`).
    assert_eq!(
        decode(EventFields {
            message: ralphy_adapter_support::API_DEGRADED_MSG.into(),
            ..Default::default()
        }),
        Some(RunEvent::ApiDegraded)
    );
    assert_eq!(
        decode(EventFields {
            message: ralphy_adapter_support::API_RECOVERED_MSG.into(),
            ..Default::default()
        }),
        Some(RunEvent::ApiRecovered)
    );
}

#[test]
fn decoder_maps_sleep_and_deadline_events() {
    assert_eq!(
        decode(EventFields {
            message: "usage limit — waiting for reset".into(),
            reset: Some("14:30".into()),
            target_epoch: Some(1_700_000_000),
            ..Default::default()
        }),
        Some(RunEvent::SleepStarted {
            reset: "14:30".into(),
            target_epoch: 1_700_000_000
        })
    );
    assert_eq!(
        decode(EventFields {
            message: "reset reached — resuming".into(),
            ..Default::default()
        }),
        Some(RunEvent::SleepEnded)
    );
    assert_eq!(
        decode(EventFields {
            message: "deadline passed — not starting issue".into(),
            number: Some(7),
            ..Default::default()
        }),
        Some(RunEvent::DeadlinePassed { number: 7 })
    );
}

#[test]
fn decoder_level_wins_warn_and_error_emit_notice() {
    // WARN: level wins even when message matches a known INFO shape.
    let result = decode(EventFields {
        level: Level::WARN,
        message: "queue built".into(),
        count: Some(3),
        order: Some("#1 -> #2 -> #3".into()),
        ..Default::default()
    });
    assert_eq!(
        result,
        Some(RunEvent::Notice {
            level: Level::WARN,
            message: "queue built".into()
        })
    );
    // ERROR: same treatment.
    let result = decode(EventFields {
        level: Level::ERROR,
        message: "something bad happened".into(),
        ..Default::default()
    });
    assert_eq!(
        result,
        Some(RunEvent::Notice {
            level: Level::ERROR,
            message: "something bad happened".into()
        })
    );
}

#[test]
fn decoder_maps_knowledge_consolidation_events() {
    assert_eq!(
        decode(EventFields {
            message: "consolidating knowledge".into(),
            count: Some(4),
            ..Default::default()
        }),
        Some(RunEvent::KnowledgeConsolidating { notes: 4 })
    );
    assert_eq!(
        decode(EventFields {
            message: "knowledge consolidated".into(),
            count: Some(4),
            ..Default::default()
        }),
        Some(RunEvent::KnowledgeConsolidated { archived: 4 })
    );
}

#[test]
fn decoder_maps_run_boundary_events() {
    // `run started`: the CLI-only parameters decode into the typed variant, and
    // a `0.0` deadline sentinel folds back to `None`.
    assert_eq!(
        decode(EventFields {
            message: "run started".into(),
            repo: Some("o/r".into()),
            queue_labels: Some("AFK, ready".into()),
            agent: Some("claude".into()),
            plan_agent: Some("claude".into()),
            branch_mode: Some("new".into()),
            base: Some("origin/main".into()),
            deadline_hours: Some(0.0),
            ..Default::default()
        }),
        Some(RunEvent::RunStarted {
            repo: "o/r".into(),
            queue_labels: vec!["AFK".into(), "ready".into()],
            agent: "claude".into(),
            plan_agent: "claude".into(),
            branch_mode: "new".into(),
            branch: "origin/main".into(),
            deadline_hours: None,
        })
    );
    // A non-zero deadline survives.
    let decoded = decode(EventFields {
        message: "run started".into(),
        deadline_hours: Some(6.0),
        ..Default::default()
    });
    assert!(matches!(
        decoded,
        Some(RunEvent::RunStarted { deadline_hours: Some(h), .. }) if (h - 6.0).abs() < 1e-9
    ));

    // `run finished`: outcome + totals decode into the typed variant.
    assert_eq!(
        decode(EventFields {
            message: "run finished".into(),
            outcome: Some("completed".into()),
            issues_done: Some(3),
            issues_skipped: Some(1),
            issues_total: Some(5),
            up: Some(100),
            cr: Some(200),
            cw: Some(50),
            out: Some(25),
            duration_s: Some(412),
            ..Default::default()
        }),
        Some(RunEvent::RunFinished {
            outcome: "completed".into(),
            issues_done: 3,
            issues_skipped: 1,
            issues_total: 5,
            issues_blocked: 0,
            issues_hitl: 0,
            issues: serde_json::Value::Null,
            up: 100,
            cr: 200,
            cw: 50,
            out: 25,
            duration_s: 412,
        })
    );
}

#[test]
fn decoder_maps_plan_snapshot_events_and_apply_is_noop() {
    // `plan opened`/`plan closed` decode into the raw-snapshot variants carrying
    // the plan_md field (arriving via record_str here).
    assert_eq!(
        decode(EventFields {
            message: "plan opened".into(),
            number: Some(7),
            plan_md: Some("# Plan\n## Steps\n- [ ] a\n".into()),
            ..Default::default()
        }),
        Some(RunEvent::PlanOpened {
            number: 7,
            plan_md: "# Plan\n## Steps\n- [ ] a\n".into(),
        })
    );
    assert_eq!(
        decode(EventFields {
            message: "plan closed".into(),
            number: Some(7),
            plan_md: Some("# Plan\n- [x] a\n".into()),
            ..Default::default()
        }),
        Some(RunEvent::PlanClosed {
            number: 7,
            plan_md: "# Plan\n- [x] a\n".into(),
        })
    );
    // Folding either snapshot is a no-op on the card model.
    let mut before = crate::runstate::RunState::new("t", 1);
    before.apply(RunEvent::IssueStarted {
        number: 7,
        title: "a".into(),
    });
    let mut after = before.clone();
    after.apply(RunEvent::PlanOpened {
        number: 7,
        plan_md: "x".into(),
    });
    after.apply(RunEvent::PlanClosed {
        number: 7,
        plan_md: "y".into(),
    });
    assert_eq!(before, after);
}

#[test]
fn parse_steps_json_maps_array_and_tolerates_absence() {
    assert_eq!(
        parse_steps_json(Some(
            r#"[{"text":"a","status":"open"},{"text":"b","status":"checked"}]"#
        )),
        vec![("a".into(), "open".into()), ("b".into(), "checked".into())]
    );
    assert!(parse_steps_json(None).is_empty());
    assert!(parse_steps_json(Some("not json")).is_empty());
}

#[test]
fn decoder_unknown_info_message_returns_none() {
    assert_eq!(
        decode(EventFields {
            message: "some unrelated log line".into(),
            ..Default::default()
        }),
        None
    );
}
