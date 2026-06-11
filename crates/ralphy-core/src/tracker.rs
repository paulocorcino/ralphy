//! Closing a green issue is a side effect on the issue tracker. Isolating it
//! behind a trait — the way [`crate::Agent`] isolates the agent CLI — lets the
//! whole queue loop be exercised in tests without touching `gh`.

use std::path::PathBuf;

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

    /// Post a free-form comment on an issue (handoff at close, skip reasoning
    /// on an infeasible plan). Default no-op so non-`gh` implementations do
    /// not need to override it.
    fn comment(&self, number: u64, body: &str) -> Result<()> {
        let _ = (number, body);
        Ok(())
    }

    /// Fetch the handoff a closed issue left behind (the last comment carrying
    /// a `## Handoff` heading), or `None` when it left none. Default `None` so
    /// non-`gh` implementations never feed handoffs.
    fn handoff_comment(&self, number: u64) -> Result<Option<String>> {
        let _ = number;
        Ok(None)
    }
}

/// The production tracker: closes issues and writes acceptance evidence through
/// the `gh` CLI, pinned to the `--repo` target so every call hits the right
/// repository regardless of the process's working directory.
pub struct GhTracker {
    repo_root: PathBuf,
}

impl GhTracker {
    /// Create a tracker whose `gh` calls run against `repo_root` (the `--repo`
    /// target), not the process cwd.
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
        }
    }
}

impl IssueTracker for GhTracker {
    fn close(&self, number: u64, comment: &str) -> Result<()> {
        github::close_issue(number, comment, &self.repo_root)
    }

    fn write_evidence(&self, number: u64, body: &str, verdicts: &[Verdict]) -> Result<()> {
        let tick = acceptance::apply_ledger(body, verdicts);
        if !tick.ticked.is_empty() {
            github::edit_issue_body(number, &tick.new_body, &self.repo_root)?;
        }
        let comment = acceptance::evidence_comment(verdicts, &tick.unmatched);
        github::comment_issue(number, &comment, &self.repo_root)
    }

    fn is_closed(&self, number: u64) -> Result<bool> {
        github::issue_is_closed(number, &self.repo_root)
    }

    fn comment(&self, number: u64, body: &str) -> Result<()> {
        github::comment_issue(number, body, &self.repo_root)
    }

    fn handoff_comment(&self, number: u64) -> Result<Option<String>> {
        let comments = github::issue_comments(number, &self.repo_root)?;
        Ok(crate::handoff::find_handoff_comment(&comments))
    }
}
