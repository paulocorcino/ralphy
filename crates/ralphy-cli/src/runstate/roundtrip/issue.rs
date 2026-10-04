//! Round trips of the issue lifecycle events and the skip reasons.

use super::*;

#[test]
fn roundtrip_issue_started() {
    let ev = one(|| ralphy_core::emit::issue_started(7, "a title"));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::IssueStarted {
            number: 7,
            title: "a title".into(),
        })
    );
}

#[test]
fn roundtrip_plan_written() {
    let ev = one(|| {
        ralphy_core::emit::plan_written(7, 3, &usage(), r#"[{"text":"a","status":"open"}]"#)
    });
    assert_eq!(
        decode(&ev),
        Some(RunEvent::PlanWritten {
            number: 7,
            open_steps: 3,
            usage: usage_lite(),
            steps: vec![("a".into(), "open".into())],
        })
    );
}

#[test]
fn roundtrip_plan_opened() {
    let ev = one(|| ralphy_core::emit::plan_opened(7, "# Plan\n## Steps\n- [ ] a\n"));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::PlanOpened {
            number: 7,
            plan_md: "# Plan\n## Steps\n- [ ] a\n".into(),
        })
    );
}

#[test]
fn roundtrip_plan_closed() {
    let ev = one(|| ralphy_core::emit::plan_closed(7, "# Plan\n- [x] a\n"));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::PlanClosed {
            number: 7,
            plan_md: "# Plan\n- [x] a\n".into(),
        })
    );
}

#[test]
fn roundtrip_issue_closed() {
    let ev = one(|| ralphy_core::emit::issue_closed(7, 1_200_000, 3, &usage()));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::IssueClosed {
            number: 7,
            tokens: 1_200_000,
            invocations: 3,
            usage: usage_lite(),
        })
    );
}

#[test]
fn roundtrip_needs_split() {
    let ev = one(|| ralphy_core::emit::needs_split(7));
    assert_eq!(decode(&ev), Some(RunEvent::NeedsSplit { number: 7 }));
}

#[test]
fn roundtrip_blocked_by_open() {
    let ev = one(|| ralphy_core::emit::blocked_by_open(140, &[139]));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::Skipped {
            number: 140,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![139],
        })
    );
}

#[test]
fn roundtrip_blocked_waiting_human() {
    // `blockers` is emitted too but the `HumanBlocked` variant carries only the
    // human half — the decoder deliberately drops the rest.
    let ev = one(|| ralphy_core::emit::blocked_waiting_human(16, &[30, 18], &[30]));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::HumanBlocked {
            number: 16,
            on: vec![30],
        })
    );
}

#[test]
fn roundtrip_non_green() {
    let ev = one(|| ralphy_core::emit::non_green(7, &ralphy_core::Outcome::Stuck));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::NonGreen {
            number: 7,
            outcome: "Stuck".into(),
        })
    );
}

#[test]
fn roundtrip_deadline_passed() {
    let ev = one(|| ralphy_core::emit::deadline_passed(7));
    assert_eq!(decode(&ev), Some(RunEvent::DeadlinePassed { number: 7 }));
}

#[test]
fn roundtrip_stop_before_label() {
    let ev = one(|| ralphy_core::emit::stop_before_label(8));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::Skipped {
            number: 8,
            kind: SkipKind::StopBefore,
            label: None,
            blockers: vec![],
        })
    );
}

#[test]
fn roundtrip_human_return_label() {
    let ev = one(|| ralphy_core::emit::human_return_label(9, "wontfix"));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::Skipped {
            number: 9,
            kind: SkipKind::HumanReturn,
            label: Some("wontfix".into()),
            blockers: vec![],
        })
    );
}

#[test]
fn roundtrip_verify_gate_failed() {
    let ev = one(|| ralphy_core::emit::verify_gate_failed(9, "cargo test: 2 failed"));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::Skipped {
            number: 9,
            kind: SkipKind::VerifyFailed,
            label: None,
            blockers: vec![],
        })
    );
}
