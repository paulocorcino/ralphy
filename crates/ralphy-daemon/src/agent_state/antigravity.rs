//! The Antigravity map: one line of `ralphy hook status antigravity` in, one
//! fold signal out (ADR-0086 D9, ADR-0040 Amendment 4). Only the daemon folds
//! these lines, so there is no adapter copy; the fixture
//! `tests/fixtures/antigravity_agent_state_mapping.json` pins the map.

use super::fold::{Signal, StatusLine};

/// The Antigravity map, over the six events of the owned root's hooks file.
/// `SessionStart` comes with the first prompt of a new conversation, not at
/// launch, and makes that conversation the lead; a resumed conversation sends
/// none. `agy` sends no event for a permission wait or an interrupt, so no
/// line maps to `Wait` or `Interrupt`.
pub(super) fn antigravity_signal(line: &StatusLine) -> Option<Signal> {
    let signal = match line.event.as_str() {
        "SessionStart" => Signal::SessionBoundary,
        "PreInvocation" | "PreToolUse" | "PostToolUse" | "PostInvocation" => Signal::Work,
        "Stop" => Signal::TurnEnd,
        _ => return None,
    };
    Some(signal)
}
