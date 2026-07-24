//! The run's single closing summary: ONE fold of a finished [`QueueReport`] into
//! the four bucket counts and the per-issue rollup, read by BOTH the final console
//! panel and the ADR-0019 `run.finished` event (PRD #218, "one vocabulary, one
//! fold"). The bucket predicates live here and nowhere else — two independent
//! copies is exactly how the panel and the wire drift apart.

use ralphy_core::{Outcome, QueueReport, ResultStatus, SkipReason};

/// One issue's terminal line in the `run.finished` rollup, in the run's own
/// vocabulary (the runner records it; the CLI never re-derives it).
pub(crate) struct SummaryIssue {
    pub number: u64,
    pub status: &'static str,
    pub kind: Option<&'static str>,
    pub blocked_by: Vec<u64>,
}

/// The closing tallies of one run: the `run.finished` outcome label, the four
/// panel buckets, the queue length, and the per-issue rollup.
pub(crate) struct RunSummary {
    pub outcome: &'static str,
    pub done: u64,
    pub blocked: u64,
    pub skipped: u64,
    pub hitl: u64,
    pub total: u64,
    pub issues: Vec<SummaryIssue>,
}

impl RunSummary {
    /// Fold a finished report into the summary. The four bucket predicates are
    /// the panel's historical ones, kept verbatim: `issues_done`/`issues_skipped`
    /// are an existing wire contract and the panel's printed numbers must not
    /// move. The rollup, by contrast, reads the runner's recorded per-issue
    /// `status`/`skip` — richer than the buckets, and the only source that can
    /// tell `infeasible` from `needs_split` from `planned`.
    pub(crate) fn from_report(report: &QueueReport, queue_len: usize) -> RunSummary {
        let done = report
            .worked
            .iter()
            .filter(|r| r.outcome == Some(Outcome::Done))
            .count() as u64;
        let blocked = report
            .worked
            .iter()
            .filter(|r| r.outcome.is_some() && r.outcome != Some(Outcome::Done))
            .count() as u64;
        // Issues stalled on a human gate in their path (ADR-0014) get their own
        // bucket and are kept out of the generic skipped tally, mirroring how the
        // live card gives them a distinct status.
        let hitl = report
            .worked
            .iter()
            .filter(|r| r.outcome.is_none() && !r.human_blockers.is_empty())
            .count() as u64;
        let skipped = report
            .worked
            .iter()
            .filter(|r| r.outcome.is_none() && r.human_blockers.is_empty())
            .count() as u64;

        RunSummary {
            outcome: super::report::outcome_of(&report.stop),
            done,
            blocked,
            skipped,
            hitl,
            total: queue_len as u64,
            issues: report
                .worked
                .iter()
                .map(|r| SummaryIssue {
                    number: r.number,
                    status: r.status.wire(),
                    kind: r.skip.map(|s| SkipReason::wire(&s)),
                    blocked_by: r.blocked_by.clone(),
                })
                .collect(),
        }
    }

    /// The rollup as the `issues_json` field of `run.finished`. `kind` rides only
    /// a skip that has one and `blocked_by` only a skip, mirroring the shape the
    /// envelope has always published. `"[]"` for a run that worked no issue.
    pub(crate) fn issues_json(&self) -> String {
        let arr: Vec<serde_json::Value> = self
            .issues
            .iter()
            .map(|i| {
                let mut o = serde_json::Map::new();
                o.insert("number".into(), i.number.into());
                o.insert("status".into(), i.status.into());
                if let Some(k) = i.kind {
                    o.insert("kind".into(), k.into());
                }
                if i.status == ResultStatus::Skipped.wire() {
                    o.insert("blocked_by".into(), i.blocked_by.clone().into());
                }
                serde_json::Value::Object(o)
            })
            .collect();
        serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ralphy_core::{IssueResult, Usage};
    use serde_json::{json, Value};

    /// One issue result in the shape `from_report` reads.
    fn result(
        number: u64,
        outcome: Option<Outcome>,
        status: ResultStatus,
        skip: Option<SkipReason>,
        blocked_by: Vec<u64>,
        human_blockers: Vec<u64>,
    ) -> IssueResult {
        IssueResult {
            number,
            outcome,
            closed: false,
            blocked_by,
            human_blockers,
            status,
            skip,
        }
    }

    /// The fixture both this test and the envelope's coherence test fold: one of
    /// every terminal shape the runner can record.
    pub(crate) fn fixture_report() -> QueueReport {
        QueueReport {
            branch: "afk/run".into(),
            orig_branch: "main".into(),
            worked: vec![
                result(
                    1,
                    Some(Outcome::Done),
                    ResultStatus::Done,
                    None,
                    vec![],
                    vec![],
                ),
                result(
                    2,
                    Some(Outcome::Stuck),
                    ResultStatus::NonGreen,
                    None,
                    vec![],
                    vec![],
                ),
                result(
                    3,
                    None,
                    ResultStatus::Skipped,
                    Some(SkipReason::HumanReturn),
                    vec![],
                    vec![],
                ),
                result(
                    4,
                    None,
                    ResultStatus::Skipped,
                    Some(SkipReason::BlockedBy),
                    vec![9],
                    vec![],
                ),
                result(5, None, ResultStatus::Hitl, None, vec![], vec![8]),
                result(6, None, ResultStatus::Planned, None, vec![], vec![]),
            ],
            stop: None,
            commits: 0,
            undo_tag: None,
            oneline: Vec::new(),
            run_usage: Usage::default(),
            run_usage_by_model: Default::default(),
            invocations: 0,
        }
    }

    #[test]
    fn from_report_buckets_and_rollup() {
        let s = RunSummary::from_report(&fixture_report(), 7);

        assert_eq!((s.done, s.blocked, s.skipped, s.hitl), (1, 1, 3, 1));
        assert_eq!(s.total, 7);
        assert_eq!(s.outcome, "completed");

        // Compare parsed `Value`s — `json!` serializes keys alphabetically, so a
        // string comparison would pin key order, not content.
        let got: Value = serde_json::from_str(&s.issues_json()).expect("valid JSON");
        assert_eq!(
            got,
            json!([
                {"number": 1, "status": "done"},
                {"number": 2, "status": "non_green"},
                {"number": 3, "status": "skipped", "kind": "human_return", "blocked_by": []},
                {"number": 4, "status": "skipped", "kind": "blocked_by", "blocked_by": [9]},
                {"number": 5, "status": "hitl"},
                {"number": 6, "status": "planned"},
            ])
        );
    }
}
