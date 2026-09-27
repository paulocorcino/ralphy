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
