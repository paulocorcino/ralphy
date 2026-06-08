//! Closing a green issue is a side effect on the issue tracker. Isolating it
//! behind a trait — the way [`crate::Agent`] isolates the agent CLI — lets the
//! whole queue loop be exercised in tests without touching `gh`.

use anyhow::Result;

use crate::github;

/// The runner's view of the issue tracker: it only ever needs to close a green
/// issue with a comment. The issue's labels are deliberately not in scope — the
/// cycle closes, it never re-labels.
pub trait IssueTracker {
    fn close(&self, number: u64, comment: &str) -> Result<()>;
}

/// The production tracker: closes issues through the `gh` CLI.
pub struct GhTracker;

impl IssueTracker for GhTracker {
    fn close(&self, number: u64, comment: &str) -> Result<()> {
        github::close_issue(number, comment)
    }
}
