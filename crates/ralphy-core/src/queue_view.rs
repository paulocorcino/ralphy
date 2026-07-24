//! The queue as data: Ralphy's judgment of the backlog, resolved once and shared
//! (ADR-0020).
//!
//! The runner's queue loop decides, per issue, whether it is eligible, parked by a
//! human-return label (ADR-0016), gated by an open blocker (ADR-0045), or the
//! `stop-before` point where the run halts. That judgment used to live only inside
//! `run_queue`'s loop. [`resolve_queue_view`] reproduces the SAME precedence over a
//! queue as pure data — reusing the very predicates the loop uses
//! ([`crate::first_stop_before`], [`crate::human_return_label`], and the
//! `open_blockers` classifier) — so the read-only `ralphy issues` surface, the
//! enriched `queue.built` event, and the runner can never disagree. An integration
//! test (`tests/queue.rs`) drives both this resolver and a dry-run `run_queue` over
//! one fixture and asserts they agree issue-for-issue.

use anyhow::Result;
use serde::Serialize;

use crate::runner::{first_stop_before, human_return_label, open_blockers};
use crate::{Issue, IssueTracker};

/// Ralphy's verdict on one queued issue, mirroring the runner loop's precedence.
/// Serializes as the wire tokens the ADR-0020 contract lists
/// (`eligible | skipped | blocked | stop_before`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueStatus {
    /// The run would work this issue: not stop-before, not parked, not blocked.
    Eligible,
    /// A human-return label (ADR-0016) outranks the queue label — the run skips it
    /// and continues. `skip_reason` names the parking label.
    Skipped,
    /// One or more declared blockers are still open (ADR-0045) — the run skips it
    /// and continues. `blocked_by` lists the open blockers.
    Blocked,
    /// The first `stop-before` issue in queue order — the run halts BEFORE it
    /// (ADR flow control). Only the first stop-before carries this status, matching
    /// the scalar `stop_before` boundary the runner records.
    StopBefore,
}

/// One issue as the runner judges it. The field order IS the wire shape defined in
/// docs/events.md: `{number, title, labels[], queue_status, skip_reason?,
/// blocked_by[], position?}`. `skip_reason`/`position` serialize as `null` when
/// absent rather than being omitted, so the JSON key set is stable for `--fields`
/// selection and for consumers programming against a flat shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssueView {
    pub number: u64,
    pub title: String,
    pub labels: Vec<String>,
    pub queue_status: QueueStatus,
    /// The parking label on a [`QueueStatus::Skipped`] issue (ADR-0016); `None`
    /// for every other status.
    pub skip_reason: Option<String>,
    /// The open blockers gating a [`QueueStatus::Blocked`] issue (ADR-0045); empty
    /// for every other status.
    pub blocked_by: Vec<u64>,
    /// The 1-based rank of this issue among the [`QueueStatus::Eligible`] issues in
    /// queue order; `None` for every non-eligible issue.
    pub position: Option<u64>,
}

/// The whole backlog as Ralphy judges it: the queue size, the issue order, the
/// `stop-before` boundary (if any), and the per-issue verdicts. The `data` payload
/// of both the enriched `queue.built` event and `dev.ralphy.queue.snapshot`
/// (ADR-0020) is built from this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueueView {
    pub count: u64,
    pub order: Vec<u64>,
    pub stop_before: Option<u64>,
    pub issues: Vec<IssueView>,
}

/// Resolve a queue into Ralphy's judgment, reproducing `run_queue`'s per-issue
/// precedence EXACTLY (see the loop in `runner.rs`):
///
/// 1. the first `stop-before` issue not in `forced` → [`QueueStatus::StopBefore`]
///    (only the first, matching the scalar boundary);
/// 2. else a human-return label (ADR-0016) → [`QueueStatus::Skipped`] with the
///    label as `skip_reason` (a `forced` selection does NOT suppress this);
/// 3. else open blockers (ADR-0045) → [`QueueStatus::Blocked`] with `blocked_by`;
/// 4. else [`QueueStatus::Eligible`], assigned a 1-based `position` among eligibles.
///
/// `forced` is the operator's explicit selection (`--issues`/`--only-issue`), which
/// suppresses `stop-before` on exactly those numbers. Blocked resolution makes the
/// same per-issue `gh` calls (`is_closed`, `open_children`, `issue_labels`) the
/// runner makes; an `is_closed`/`open_children` failure is fatal (`Err`), exactly
/// as in the loop.
pub fn resolve_queue_view(
    queue: &[Issue],
    forced: &[u64],
    human_return_labels: &[String],
    tracker: &dyn IssueTracker,
) -> Result<QueueView> {
    let stop_before = first_stop_before(queue, forced);
    let mut next_position = 0u64;
    let mut issues = Vec::with_capacity(queue.len());
    for issue in queue {
        let mut skip_reason = None;
        let mut blocked_by = Vec::new();
        let mut position = None;
        let queue_status = if stop_before == Some(issue.number) {
            QueueStatus::StopBefore
        } else if let Some(label) = human_return_label(issue, human_return_labels) {
            skip_reason = Some(label.clone());
            QueueStatus::Skipped
        } else {
            let open = open_blockers(issue, tracker)?.open;
            if open.is_empty() {
                next_position += 1;
                position = Some(next_position);
                QueueStatus::Eligible
            } else {
                blocked_by = open;
                QueueStatus::Blocked
            }
        };
        issues.push(IssueView {
            number: issue.number,
            title: issue.title.clone(),
            labels: issue.labels.clone(),
            queue_status,
            skip_reason,
            blocked_by,
            position,
        });
    }
    Ok(QueueView {
        count: queue.len() as u64,
        order: queue.iter().map(|i| i.number).collect(),
        stop_before,
        issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::STOP_BEFORE_LABEL;
    use std::collections::{HashMap, HashSet};

    /// A read-only tracker whose blocked-by answers are scripted: `open` lists the
    /// issue numbers reported as still-open by `is_closed`; every other number is
    /// closed. `children`/`labels` script `open_children`/`issue_labels`. Any
    /// mutating call panics — the query surface must never write.
    #[derive(Default)]
    struct FakeTracker {
        open: HashSet<u64>,
        children: HashMap<u64, Vec<u64>>,
        labels: HashMap<u64, Vec<String>>,
    }

    impl IssueTracker for FakeTracker {
        fn close(&self, _n: u64, _c: &str) -> Result<()> {
            panic!("close: query surface must be read-only")
        }
        fn is_closed(&self, number: u64) -> Result<bool> {
            Ok(!self.open.contains(&number))
        }
        fn open_children(&self, number: u64) -> Result<Vec<u64>> {
            Ok(self.children.get(&number).cloned().unwrap_or_default())
        }
        fn issue_labels(&self, number: u64) -> Result<Vec<String>> {
            Ok(self.labels.get(&number).cloned().unwrap_or_default())
        }
    }

    fn issue(number: u64, labels: &[&str], body: &str) -> Issue {
        Issue {
            number,
            title: format!("issue {number}"),
            body: body.to_string(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
            comments: Vec::new(),
        }
    }

    fn human_return() -> Vec<String> {
        ["needs-info", "wontfix", "ready-for-human"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn classifies_each_status_with_reasons_and_eligible_positions() {
        // One stop-before, one human-return-parked, one blocked by an open issue,
        // and two clean — proving each verdict, its reason, and that `position` is
        // assigned only to eligibles, in order.
        let queue = vec![
            issue(1, &[STOP_BEFORE_LABEL], ""),
            issue(2, &["needs-info"], ""),
            issue(3, &[], "## Blocked by\n- #99\n"),
            issue(4, &[], ""),
            issue(5, &[], ""),
        ];
        let mut tracker = FakeTracker::default();
        tracker.open.insert(99); // blocker #99 is still open

        let view = resolve_queue_view(&queue, &[], &human_return(), &tracker).unwrap();

        assert_eq!(view.count, 5);
        assert_eq!(view.order, vec![1, 2, 3, 4, 5]);
        assert_eq!(view.stop_before, Some(1));

        let v = |n: u64| view.issues.iter().find(|i| i.number == n).unwrap();

        // #1: the first stop-before → StopBefore, no position.
        assert_eq!(v(1).queue_status, QueueStatus::StopBefore);
        assert_eq!(v(1).position, None);
        assert!(v(1).skip_reason.is_none());

        // #2: human-return label → Skipped, skip_reason names the parking label.
        assert_eq!(v(2).queue_status, QueueStatus::Skipped);
        assert_eq!(v(2).skip_reason.as_deref(), Some("needs-info"));
        assert_eq!(v(2).position, None);

        // #3: an open blocker → Blocked, blocked_by lists it.
        assert_eq!(v(3).queue_status, QueueStatus::Blocked);
        assert_eq!(v(3).blocked_by, vec![99]);
        assert_eq!(v(3).position, None);

        // #4, #5: clean → Eligible, 1-based position among eligibles.
        assert_eq!(v(4).queue_status, QueueStatus::Eligible);
        assert_eq!(v(4).position, Some(1));
        assert_eq!(v(5).queue_status, QueueStatus::Eligible);
        assert_eq!(v(5).position, Some(2));
    }

    #[test]
    fn forced_selection_suppresses_stop_before_only() {
        // Forcing the stop-before issue makes it run normally (eligible), while a
        // human-return label is NOT suppressed by forcing (ADR-0016).
        let queue = vec![
            issue(1, &[STOP_BEFORE_LABEL], ""),
            issue(2, &["wontfix"], ""),
        ];
        let tracker = FakeTracker::default();
        let view = resolve_queue_view(&queue, &[1, 2], &human_return(), &tracker).unwrap();
        assert_eq!(view.stop_before, None);
        let v = |n: u64| view.issues.iter().find(|i| i.number == n).unwrap();
        // #1 forced past its stop-before → eligible.
        assert_eq!(v(1).queue_status, QueueStatus::Eligible);
        assert_eq!(v(1).position, Some(1));
        // #2 still parked despite being forced.
        assert_eq!(v(2).queue_status, QueueStatus::Skipped);
        assert_eq!(v(2).skip_reason.as_deref(), Some("wontfix"));
    }

    #[test]
    fn issue_view_serializes_the_full_wire_key_set_in_order() {
        let queue = vec![issue(7, &["queue"], "")];
        let tracker = FakeTracker::default();
        let view = resolve_queue_view(&queue, &[], &human_return(), &tracker).unwrap();
        let json = serde_json::to_value(&view.issues[0]).unwrap();
        let obj = json.as_object().unwrap();
        // serde_json's Value stores keys sorted, so assert the field SET (order is
        // not semantically meaningful for a JSON object).
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected = vec![
            "number",
            "title",
            "labels",
            "queue_status",
            "skip_reason",
            "blocked_by",
            "position",
        ];
        expected.sort_unstable();
        assert_eq!(keys, expected);
        assert_eq!(json["queue_status"], "eligible");
        assert!(json["skip_reason"].is_null());
        assert_eq!(json["blocked_by"], serde_json::json!([]));
        assert_eq!(json["position"], 1);
    }
}
