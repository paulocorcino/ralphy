//! The ADR-0039 §2 round-trip gate: for every `ralphy_core::emit` helper, emit it
//! for real, capture the `tracing` event, and decode it back — asserting the exact
//! [`RunEvent`] AND the `INFO` level contract.
//!
//! This is what makes the vocabulary typed rather than merely centralized: a
//! helper that renames a field, flips a `%` to a `?`, or drops to `WARN` reds here
//! even though both halves still compile.

use tracing::Level;

use super::capture::{capture_events, Captured};
use super::{event_to_runevent, EventFields, RunEvent};

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
