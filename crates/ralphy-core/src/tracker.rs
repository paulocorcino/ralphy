//! Closing a green issue is a side effect on the issue tracker. Isolating it
//! behind a trait — the way [`crate::Agent`] isolates the agent CLI — lets the
//! whole queue loop be exercised in tests without touching `gh`.

use anyhow::Result;

use crate::acceptance::{self, Verdict};
use crate::github;

/// The runner's view of the issue tracker: close a green issue and write back
/// its acceptance evidence. The evidence write is optional — a default no-op
/// is provided so non-`gh` implementations do not need to override it.
pub trait IssueTracker {
    fn close(&self, number: u64, comment: &str) -> Result<()>;

    /// Tick verified AC checkboxes in the issue body and post an evidence
    /// comment. Called only when the parsed verdict list is non-empty.
    fn write_evidence(&self, number: u64, body: &str, verdicts: &[Verdict]) -> Result<()> {
        let _ = (number, body, verdicts);
        Ok(())
    }

    /// Return `true` when the given issue number is closed. The default impl
    /// returns `Ok(true)` so non-`gh` test fakes that do not override it
    /// never block on any issue.
    fn is_closed(&self, number: u64) -> Result<bool> {
        let _ = number;
        Ok(true)
    }
}

/// The production tracker: closes issues and writes acceptance evidence through
/// the `gh` CLI.
pub struct GhTracker;

impl IssueTracker for GhTracker {
    fn close(&self, number: u64, comment: &str) -> Result<()> {
        github::close_issue(number, comment)
    }

    fn write_evidence(&self, number: u64, body: &str, verdicts: &[Verdict]) -> Result<()> {
        let tick = acceptance::apply_ledger(body, verdicts);
        if !tick.ticked.is_empty() {
            github::edit_issue_body(number, &tick.new_body)?;
        }
        let comment = acceptance::evidence_comment(verdicts, &tick.unmatched);
        github::comment_issue(number, &comment)
    }

    fn is_closed(&self, number: u64) -> Result<bool> {
        github::issue_is_closed(number)
    }
}
