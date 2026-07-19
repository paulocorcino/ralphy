//! The typed run-event vocabulary (ADR-0039 §1): one `pub fn` per consumed
//! lifecycle event, owning its message, its field names, their `%`/`?` encoding,
//! and its level.
//!
//! Nothing outside this module may write one of these message literals — the
//! `…_MSG` constants are the single source, and the CLI decoder
//! (`crates/ralphy-cli/src/runstate/event.rs`) matches against them rather than
//! against strings.
//!
//! **The convention**: a new `RunEvent` variant without an `emit` helper AND a
//! round-trip test is an incomplete change (ADR-0039 §2). The round-trip lives in
//! `crates/ralphy-cli/src/runstate/roundtrip.rs`.
//!
//! Every helper emits at `INFO` on purpose: the decoder short-circuits
//! `WARN`/`ERROR` into a generic `Notice`, so a helper logged above `INFO` would
//! silently lose its identity.
//!
//! The message is passed as `"{}", MSG` rather than as a literal because
//! `format_args!` takes only literals — the constant, not a copy of its text, is
//! what every helper emits.

use tracing::info;

/// See [`issue_started`].
pub const ISSUE_STARTED_MSG: &str = "issue started";

/// Work began on an issue.
pub fn issue_started(number: u64, title: &str) {
    info!(number, title = %title, "{}", ISSUE_STARTED_MSG);
}
