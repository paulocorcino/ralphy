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
        RunEvent::RunStopped { .. } => "roundtrip_run_stopped",
        RunEvent::SleepStarted { .. } => "roundtrip_usage_limit_waiting",
        RunEvent::SleepEnded => "roundtrip_reset_reached",
        RunEvent::IdleReaped { .. } => "roundtrip_idle_reaped",
        RunEvent::AgentState { .. } => "roundtrip_agent_state",
        RunEvent::ApiDegraded => "roundtrip_api_degraded",
        RunEvent::ApiRecovered => "roundtrip_api_recovered",
        RunEvent::KnowledgeConsolidating { .. } => "roundtrip_knowledge_consolidating",
        RunEvent::KnowledgeConsolidated { .. } => "roundtrip_knowledge_consolidated",
        RunEvent::RunStarted { .. } => "roundtrip_run_started",
        RunEvent::RunFinished { .. } => "roundtrip_run_finished",
        RunEvent::RunSkipped { .. } => "roundtrip_run_skipped",
        RunEvent::Notice { .. } => "roundtrip_level_wins_over_message",
        RunEvent::Planning { .. } => "roundtrip_planning",
        RunEvent::Executing { .. } => "roundtrip_executing",
    }
}

#[test]
fn roundtrip_level_wins_over_message() {
    // The other half of the level contract: a message emitted above INFO does
    // NOT decode to its variant — it collapses to a `Notice`, even when the
    // message and its fields match a known INFO shape. This is why `one`
    // asserts INFO for every helper.
    // (case, level, message, fields)
    let rows = [
        (
            "WARN vocabulary message",
            Level::WARN,
            ralphy_core::emit::ISSUE_STARTED_MSG,
            EventFields::default(),
        ),
        (
            "WARN message with its INFO fields",
            Level::WARN,
            "queue built",
            EventFields {
                count: Some(3),
                order: Some("#1 -> #2 -> #3".into()),
                ..Default::default()
            },
        ),
        (
            "ERROR message",
            Level::ERROR,
            "something bad happened",
            EventFields::default(),
        ),
    ];
    for (case, level, message, fields) in rows {
        assert_eq!(
            event_to_runevent(
                "ralphy_core::emit",
                message,
                &EventFields {
                    level,
                    message: message.into(),
                    ..fields
                },
            ),
            Some(RunEvent::Notice {
                level,
                message: message.into(),
            }),
            "{case}"
        );
    }
}

mod issue;
mod phase;
mod run_level;
