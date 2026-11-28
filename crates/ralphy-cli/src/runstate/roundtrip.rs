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
/// [`RunEvent`] variant maps to the round-trip test that proves it, as `test` or
/// `test/row` for a row of a table test (`simple/` is `roundtrip_simple_run_events/`).
/// No wildcard arm — a variant added without one fails to compile until someone lists it,
/// which is the ADR-0039 §2 convention made mechanical.
fn _every_variant_has_a_roundtrip(e: &RunEvent) -> &'static str {
    match e {
        RunEvent::QueueBuilt { .. } => "roundtrip_queue_built",
        RunEvent::IssueStarted { .. } => "roundtrip_issue_lifecycle/issue_started",
        RunEvent::PlanWritten { .. } => "roundtrip_issue_lifecycle/plan_written",
        RunEvent::PlanOpened { .. } => "roundtrip_issue_lifecycle/plan_opened",
        RunEvent::PlanClosed { .. } => "roundtrip_issue_lifecycle/plan_closed",
        RunEvent::IssueClosed { .. } => "roundtrip_issue_lifecycle/issue_closed",
        RunEvent::NonGreen { .. } => "roundtrip_issue_lifecycle/non_green",
        RunEvent::NeedsSplit { .. } => "roundtrip_issue_lifecycle/needs_split",
        RunEvent::Skipped { .. } => "roundtrip_skip_reasons (all four Skipped rows)",
        RunEvent::HumanBlocked { .. } => "roundtrip_skip_reasons/blocked_waiting_human",
        RunEvent::DeadlinePassed { .. } => "roundtrip_issue_lifecycle/deadline_passed",
        RunEvent::RunStopped { .. } => "roundtrip_run_stopped",
        RunEvent::SleepStarted { .. } => "simple/usage_limit_waiting",
        RunEvent::SleepEnded => "simple/reset_reached",
        RunEvent::IdleReaped { .. } => "simple/idle_reaped",
        RunEvent::HooksUnprovenAllowed { .. } => "simple/hooks_unproven_allowed",
        RunEvent::AgentState { .. } => "simple/agent_state",
        RunEvent::ApiDegraded => "simple/api_degraded",
        RunEvent::ApiRecovered => "simple/api_recovered",
        RunEvent::KnowledgeConsolidating { .. } => "simple/knowledge_consolidating",
        RunEvent::KnowledgeConsolidated { .. } => "simple/knowledge_consolidated",
        RunEvent::RunStarted { .. } => "roundtrip_run_started",
        RunEvent::RunFinished { .. } => "roundtrip_run_finished",
        RunEvent::RunSkipped { .. } => "simple/run_skipped",
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
