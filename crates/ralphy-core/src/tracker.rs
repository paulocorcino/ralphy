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

    /// Add a label to an issue (e.g. `needs-split` on a bundle verdict).
    /// Default no-op so non-`gh` implementations never touch labels.
    fn add_label(&self, number: u64, label: &str) -> Result<()> {
        let _ = (number, label);
        Ok(())
    }

    /// Remove a label from an issue — the label swaps `ralphy triage` performs
    /// (ADR-0017). Default no-op so non-`gh` implementations never touch labels.
    fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        let _ = (number, label);
        Ok(())
    }

    /// Post-or-edit the single comment carrying `marker` (ADR-0017): the
    /// consolidated-spec comment `ralphy triage` maintains, idempotent by
    /// construction so re-triage edits its own comment rather than stacking a
    /// second one. Default no-op so non-`gh` implementations never comment.
    fn upsert_marked_comment(&self, number: u64, marker: &str, body: &str) -> Result<()> {
        let _ = (number, marker, body);
        Ok(())
    }

    /// Fetch the handoff a closed issue left behind (the last comment carrying
    /// a `## Handoff` heading), or `None` when it left none. Default `None` so
    /// non-`gh` implementations never feed handoffs.
    fn handoff_comment(&self, number: u64) -> Result<Option<String>> {
        let _ = number;
        Ok(None)
    }

    /// Fetch an issue's comment bodies in thread order — the discussion the
    /// runner attaches to the selected issue before planning, so the planner
    /// and executor read it alongside the body. Default empty so non-`gh`
    /// implementations never feed comments.
    fn issue_comments(&self, number: u64) -> Result<Vec<String>> {
        let _ = number;
        Ok(Vec::new())
    }

    /// Fetch the source of an issue named in a STRUCTURED reference section
    /// (`## Blocked by` / `## Parent`) of the issue being planned: its title,
    /// state, and body, so the planner reads the referenced spec instead of
    /// paraphrasing a `#N` mention into a child issue. Default `None` so non-`gh`
    /// implementations feed no references.
    fn reference(&self, number: u64) -> Result<Option<crate::references::Reference>> {
        let _ = number;
        Ok(None)
    }

    /// The OPEN issues whose `## Parent` section references `number` — the
    /// live children of a retired bundle. A closed blocker with open children
    /// still blocks: its work moved into the children, so the dependent must
    /// wait for them. Default empty so non-`gh` implementations keep the
    /// plain closed-means-done gate.
    fn open_children(&self, number: u64) -> Result<Vec<u64>> {
        let _ = number;
        Ok(Vec::new())
    }

    /// The labels on an open blocker, so the blocked-by gate can tell a human
    /// gate (`ready-for-human`/`HITL`, parked until a person acts — ADR-0014)
    /// apart from ordinary agent work the queue will clear on its own. Default
    /// empty so non-`gh` implementations classify every blocker as agent work.
    fn issue_labels(&self, number: u64) -> Result<Vec<String>> {
        let _ = number;
        Ok(Vec::new())
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

    fn add_label(&self, number: u64, label: &str) -> Result<()> {
        github::add_label(number, label, &self.repo_root)
    }

    fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        github::remove_label(number, label, &self.repo_root)
    }

    fn upsert_marked_comment(&self, number: u64, marker: &str, body: &str) -> Result<()> {
        github::upsert_marked_comment(number, marker, body, &self.repo_root)
    }

    fn handoff_comment(&self, number: u64) -> Result<Option<String>> {
        let comments = github::issue_comments(number, &self.repo_root)?;
        Ok(crate::handoff::find_handoff_comment(&comments))
    }

    fn issue_comments(&self, number: u64) -> Result<Vec<String>> {
        github::issue_comments(number, &self.repo_root)
    }

    fn reference(&self, number: u64) -> Result<Option<crate::references::Reference>> {
        github::fetch_reference(number, &self.repo_root).map(Some)
    }

    fn open_children(&self, number: u64) -> Result<Vec<u64>> {
        let open = github::list_open_issues(&self.repo_root)?;
        Ok(open
            .iter()
            .filter(|i| crate::blocked::parse_parent(&i.body).contains(&number))
            .map(|i| i.number)
            .collect())
    }

    fn issue_labels(&self, number: u64) -> Result<Vec<String>> {
        github::issue_labels(number, &self.repo_root)
    }
}
