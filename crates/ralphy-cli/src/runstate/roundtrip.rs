//! The ADR-0039 §2 round-trip gate: for every `ralphy_core::emit` helper, emit it
//! for real, capture the `tracing` event, and decode it back — asserting the exact
//! [`RunEvent`] AND the `INFO` level contract.
//!
//! This is what makes the vocabulary typed rather than merely centralized: a
//! helper that renames a field, flips a `%` to a `?`, or drops to `WARN` reds here
//! even though both halves still compile.

use tracing::Level;

use super::capture::{capture_events, Captured};
use super::{event_to_runevent, EventFields, RunEvent, SkipKind, UsageLite};

/// A `Usage` with a distinct value per slot, so a helper that swaps two of them
/// (`cache_read` for `cache_creation`) reds rather than passing on symmetry.
fn usage() -> ralphy_core::Usage {
    ralphy_core::Usage {
        input: 11,
        cache_read: 22,
        cache_creation: 33,
        output: 44,
        model: Some("claude-opus-4".into()),
    }
}

/// The [`UsageLite`] the decoder must read back out of [`usage`].
fn usage_lite() -> UsageLite {
    UsageLite {
        input: 11,
        cache_read: 22,
        cache_creation: 33,
        output: 44,
        model: Some("claude-opus-4".into()),
    }
}

/// Run one emit helper and hand back its single captured event, asserting the
/// half of the contract every helper shares: exactly one event, at `INFO`.
fn one(f: impl FnOnce()) -> Captured {
    let ((), mut events) = capture_events(f);
    assert_eq!(events.len(), 1, "exactly one event per emit helper");
    let ev = events.remove(0);
    assert_eq!(
        ev.level,
        Level::INFO,
        "`{}` must be emitted at INFO — the decoder collapses WARN/ERROR into a generic Notice",
        ev.message
    );
    ev
}

/// Decode a captured event through the production decoder.
fn decode(ev: &Captured) -> Option<RunEvent> {
    event_to_runevent(&ev.target, &ev.message, &ev.fields)
}

/// The coverage closure, enforced by the COMPILER rather than by a count: every
/// [`RunEvent`] variant maps to the round-trip test that proves it. No wildcard
/// arm — a variant added without one fails to compile until someone lists it,
/// which is the ADR-0039 §2 convention made mechanical.
#[allow(dead_code)]
fn _every_variant_has_a_roundtrip(e: &RunEvent) -> &'static str {
    match e {
        RunEvent::QueueBuilt { .. } => "roundtrip_queue_built",
        RunEvent::IssueStarted { .. } => "roundtrip_issue_started",
        RunEvent::PlanWritten { .. } => "roundtrip_plan_written",
        RunEvent::PlanOpened { .. } => "roundtrip_plan_opened",
        RunEvent::PlanClosed { .. } => "roundtrip_plan_closed",
        RunEvent::IssueClosed { .. } => "roundtrip_issue_closed",
        RunEvent::NonGreen { .. } => "roundtrip_non_green",
        RunEvent::NeedsSplit { .. } => "roundtrip_needs_split",
        RunEvent::Skipped { .. } => {
            "roundtrip_{blocked_by_open,stop_before_label,human_return_label,verify_gate_failed}"
        }
        RunEvent::HumanBlocked { .. } => "roundtrip_blocked_waiting_human",
        RunEvent::DeadlinePassed { .. } => "roundtrip_deadline_passed",
        RunEvent::SleepStarted { .. } => "roundtrip_usage_limit_waiting",
        RunEvent::SleepEnded => "roundtrip_reset_reached",
        RunEvent::IdleReaped { .. } => "roundtrip_idle_reaped",
        RunEvent::ApiDegraded => "roundtrip_api_degraded",
        RunEvent::ApiRecovered => "roundtrip_api_recovered",
        RunEvent::KnowledgeConsolidating { .. } => "roundtrip_knowledge_consolidating",
        RunEvent::KnowledgeConsolidated { .. } => "roundtrip_knowledge_consolidated",
        RunEvent::RunStarted { .. } => "roundtrip_run_started",
        RunEvent::RunFinished { .. } => "roundtrip_run_finished",
        RunEvent::Notice { .. } => "roundtrip_level_wins_over_message",
        RunEvent::Planning { .. } | RunEvent::Executing { .. } => {
            "deferred to Fase 1b — the per-adapter phase strings still own these"
        }
    }
}

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
    let ev = one(|| ralphy_core::emit::issue_closed(7, 1_200_000, &usage()));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::IssueClosed {
            number: 7,
            tokens: 1_200_000,
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

#[test]
fn roundtrip_usage_limit_waiting() {
    // `hint` rides along for the log but has no decoded home — the pin in
    // `ralphy-core`'s `pins_usage_limit_vocabulary` is what keeps it emitted.
    let ev = one(|| {
        ralphy_core::emit::usage_limit_waiting("07:30", "2026-07-19T07:25:00Z", 1_700_000_000)
    });
    assert_eq!(
        decode(&ev),
        Some(RunEvent::SleepStarted {
            reset: "07:30".into(),
            target_epoch: 1_700_000_000,
        })
    );
}

#[test]
fn roundtrip_reset_reached() {
    let ev = one(ralphy_core::emit::reset_reached);
    assert_eq!(decode(&ev), Some(RunEvent::SleepEnded));
}

#[test]
fn roundtrip_idle_reaped() {
    let ev = one(|| ralphy_core::emit::idle_reaped(20));
    assert_eq!(decode(&ev), Some(RunEvent::IdleReaped { idle_minutes: 20 }));
}

#[test]
fn roundtrip_api_degraded() {
    let ev = one(ralphy_core::emit::api_degraded);
    assert_eq!(decode(&ev), Some(RunEvent::ApiDegraded));
}

#[test]
fn roundtrip_api_recovered() {
    let ev = one(ralphy_core::emit::api_recovered);
    assert_eq!(decode(&ev), Some(RunEvent::ApiRecovered));
}

#[test]
fn roundtrip_queue_built() {
    let ev = one(|| {
        ralphy_core::emit::queue_built(
            3,
            "#1 -> #2 -> #3",
            2,
            r#"[{"number":1,"queue_status":"eligible"}]"#,
            "octocat",
        )
    });
    assert_eq!(
        decode(&ev),
        Some(RunEvent::QueueBuilt {
            count: 3,
            order: vec![1, 2, 3],
            stop_before: Some(2),
            issues: serde_json::json!([{"number":1,"queue_status":"eligible"}]),
            assignee_filter: Some("octocat".into()),
        })
    );
}

#[test]
fn roundtrip_run_started() {
    let ev = one(|| {
        ralphy_core::emit::run_started(
            "o/r",
            "AFK,ready",
            "claude",
            "codex",
            "new",
            "origin/main",
            6.0,
        )
    });
    assert_eq!(
        decode(&ev),
        Some(RunEvent::RunStarted {
            repo: "o/r".into(),
            queue_labels: vec!["AFK".into(), "ready".into()],
            agent: "claude".into(),
            plan_agent: "codex".into(),
            branch_mode: "new".into(),
            branch: "origin/main".into(),
            deadline_hours: Some(6.0),
        })
    );
}

#[test]
fn roundtrip_run_finished() {
    let ev = one(|| ralphy_core::emit::run_finished("completed", 3, 1, 5, &usage(), 412));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::RunFinished {
            outcome: "completed".into(),
            issues_done: 3,
            issues_skipped: 1,
            issues_total: 5,
            up: 11,
            cr: 22,
            cw: 33,
            out: 44,
            duration_s: 412,
        })
    );
    // A run spans models, so `run finished` deliberately carries no `model`.
    assert_eq!(ev.fields.model, None);
}

#[test]
fn roundtrip_knowledge_consolidating() {
    let ev = one(|| ralphy_core::emit::knowledge_consolidating(4));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::KnowledgeConsolidating { notes: 4 })
    );
}

#[test]
fn roundtrip_knowledge_consolidated() {
    let ev = one(|| ralphy_core::emit::knowledge_consolidated(4));
    assert_eq!(
        decode(&ev),
        Some(RunEvent::KnowledgeConsolidated { archived: 4 })
    );
}

#[test]
fn roundtrip_level_wins_over_message() {
    // The other half of the level contract: a vocabulary message emitted above
    // INFO does NOT decode to its variant — it collapses to a `Notice`. This is
    // why `one` asserts INFO for every helper.
    assert_eq!(
        event_to_runevent(
            "ralphy_core::emit",
            ralphy_core::emit::ISSUE_STARTED_MSG,
            &EventFields {
                level: Level::WARN,
                ..Default::default()
            },
        ),
        Some(RunEvent::Notice {
            level: Level::WARN,
            message: "issue started".into(),
        })
    );
}
