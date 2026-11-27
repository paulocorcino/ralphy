//! Round trips of the issue lifecycle events and the skip reasons.

use super::*;

/// One emit helper and the event the decoder must read back from it.
type Row = (&'static str, fn(), RunEvent);

fn assert_rows(rows: Vec<Row>) {
    for (name, emit, expected) in rows {
        let ev = one(emit);
        assert_eq!(decode(&ev), Some(expected), "{name}");
    }
}

#[test]
fn roundtrip_issue_lifecycle() {
    let rows: Vec<Row> = vec![
        (
            "issue_started",
            || ralphy_core::emit::issue_started(7, "a title"),
            RunEvent::IssueStarted {
                number: 7,
                title: "a title".into(),
            },
        ),
        (
            "plan_written",
            || {
                ralphy_core::emit::plan_written(
                    7,
                    3,
                    &usage(),
                    r#"[{"text":"a","status":"open"}]"#,
                );
            },
            RunEvent::PlanWritten {
                number: 7,
                open_steps: 3,
                usage: usage_lite(),
                steps: vec![("a".into(), "open".into())],
            },
        ),
        (
            "plan_opened",
            || ralphy_core::emit::plan_opened(7, "# Plan\n## Steps\n- [ ] a\n"),
            RunEvent::PlanOpened {
                number: 7,
                plan_md: "# Plan\n## Steps\n- [ ] a\n".into(),
            },
        ),
        (
            "plan_closed",
            || ralphy_core::emit::plan_closed(7, "# Plan\n- [x] a\n"),
            RunEvent::PlanClosed {
                number: 7,
                plan_md: "# Plan\n- [x] a\n".into(),
            },
        ),
        (
            "issue_closed",
            || ralphy_core::emit::issue_closed(7, 1_200_000, 3, &usage()),
            RunEvent::IssueClosed {
                number: 7,
                tokens: 1_200_000,
                invocations: 3,
                usage: usage_lite(),
            },
        ),
        (
            "needs_split",
            || ralphy_core::emit::needs_split(7),
            RunEvent::NeedsSplit { number: 7 },
        ),
        (
            "non_green",
            || ralphy_core::emit::non_green(7, &ralphy_core::Outcome::Stuck),
            RunEvent::NonGreen {
                number: 7,
                outcome: "Stuck".into(),
            },
        ),
        (
            "deadline_passed",
            || ralphy_core::emit::deadline_passed(7),
            RunEvent::DeadlinePassed { number: 7 },
        ),
    ];
    assert_rows(rows);
}

#[test]
fn roundtrip_skip_reasons() {
    let rows: Vec<Row> = vec![
        (
            "blocked_by_open",
            || ralphy_core::emit::blocked_by_open(140, &[139]),
            RunEvent::Skipped {
                number: 140,
                kind: SkipKind::BlockedBy,
                label: None,
                blockers: vec![139],
            },
        ),
        (
            // `blockers` is emitted too but the `HumanBlocked` variant carries
            // only the human half — the decoder deliberately drops the rest.
            "blocked_waiting_human",
            || ralphy_core::emit::blocked_waiting_human(16, &[30, 18], &[30]),
            RunEvent::HumanBlocked {
                number: 16,
                on: vec![30],
            },
        ),
        (
            "stop_before_label",
            || ralphy_core::emit::stop_before_label(8),
            RunEvent::Skipped {
                number: 8,
                kind: SkipKind::StopBefore,
                label: None,
                blockers: vec![],
            },
        ),
        (
            "human_return_label",
            || ralphy_core::emit::human_return_label(9, "wontfix"),
            RunEvent::Skipped {
                number: 9,
                kind: SkipKind::HumanReturn,
                label: Some("wontfix".into()),
                blockers: vec![],
            },
        ),
        (
            "verify_gate_failed",
            || ralphy_core::emit::verify_gate_failed(9, "cargo test: 2 failed"),
            RunEvent::Skipped {
                number: 9,
                kind: SkipKind::VerifyFailed,
                label: None,
                blockers: vec![],
            },
        ),
    ];
    assert_rows(rows);
}
