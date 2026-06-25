//! The backlog/milestone → issues preview-draft schema (ADR-0012 stage 8). A
//! judgment agent session reads the repo's backlog or milestone docs and emits an
//! [`IssuesDraft`] against this Rust-defined schema — a LOCAL preview, never a
//! direct GitHub write. `ralphy init` then summarizes it, the dev confirms, and
//! only then does the cli publish via `gh`. The schema lives in core (next to
//! [`crate::DiagnosisReport`]) because it is a domain artifact shared by the agent
//! crate (which produces it) and the cli (which consumes and publishes it).

use serde::{Deserialize, Serialize};

/// One drafted GitHub issue — titles, labels, milestone link, and the full body,
/// none of it published yet. `blocked_by` references EARLIER drafts by their
/// 0-based position in [`IssuesDraft::issues`], not by issue number, because the
/// numbers do not exist until publish; the cli maps index → created number when it
/// publishes in array order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDraft {
    /// Short descriptive title.
    pub title: String,
    /// The full issue body (the tracer-bullet template, with a "Blocked by"
    /// section the cli rewrites with real `#N` refs at publish time).
    pub body: String,
    /// Labels to apply, including the triage label that makes it agent-ready.
    pub labels: Vec<String>,
    /// 0-based indices of earlier drafts in [`IssuesDraft::issues`] this one is
    /// blocked by. Empty when it can start immediately.
    #[serde(default)]
    pub blocked_by: Vec<usize>,
}

/// A drafted GitHub Milestone (the milestone path only). The cli creates it first,
/// then links each issue to it at publish time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MilestoneDraft {
    /// The milestone title issues are linked to.
    pub title: String,
    /// The milestone description.
    #[serde(default)]
    pub description: String,
}

/// The structured output of one judgment session: the issues to create (in
/// dependency order, blockers first), plus — on the milestone path — the milestone
/// to create and the PRD file the agent wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuesDraft {
    /// The milestone to create, `Some` only on the milestone path.
    #[serde(default)]
    pub milestone: Option<MilestoneDraft>,
    /// The PRD file the agent wrote (path relative to the repo), `Some` only on
    /// the milestone path.
    #[serde(default)]
    pub prd_path: Option<String>,
    /// The drafted issues, topologically ordered so a `blocked_by` index always
    /// points at an earlier entry.
    pub issues: Vec<IssueDraft>,
}

impl IssuesDraft {
    /// Number of issues drafted.
    pub fn issue_count(&self) -> usize {
        self.issues.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canonical milestone-path draft, pinning the wire format.
    const SAMPLE_JSON: &str = r###"{
        "milestone": { "title": "v1 onboarding", "description": "First runnable state" },
        "prd_path": "docs/prd/0001-onboarding.md",
        "issues": [
            { "title": "scaffold workspace", "body": "## What to build\n...", "labels": ["ready-for-agent"], "blocked_by": [] },
            { "title": "wire the queue", "body": "## What to build\n...", "labels": ["ready-for-agent"], "blocked_by": [0] }
        ]
    }"###;

    #[test]
    fn serde_round_trip() {
        let draft = IssuesDraft {
            milestone: Some(MilestoneDraft {
                title: "v1 onboarding".into(),
                description: "First runnable state".into(),
            }),
            prd_path: Some("docs/prd/0001-onboarding.md".into()),
            issues: vec![
                IssueDraft {
                    title: "scaffold workspace".into(),
                    body: "## What to build\n...".into(),
                    labels: vec!["ready-for-agent".into()],
                    blocked_by: vec![],
                },
                IssueDraft {
                    title: "wire the queue".into(),
                    body: "## What to build\n...".into(),
                    labels: vec!["ready-for-agent".into()],
                    blocked_by: vec![0],
                },
            ],
        };
        let json = serde_json::to_string(&draft).expect("serialize");
        let back: IssuesDraft = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(draft, back);
    }

    #[test]
    fn deserialize_sample_draft() {
        let draft: IssuesDraft = serde_json::from_str(SAMPLE_JSON).expect("parse sample");
        assert_eq!(draft.issue_count(), 2);
        assert_eq!(draft.milestone.as_ref().unwrap().title, "v1 onboarding");
        assert_eq!(draft.issues[1].blocked_by, vec![0]);
    }

    #[test]
    fn loose_backlog_draft_omits_milestone() {
        // The loose-backlog path emits issues with no milestone/prd — both default
        // to None so the agent may omit the keys entirely.
        let json = r#"{
            "issues": [
                { "title": "thin slice", "body": "...", "labels": ["ready-for-agent"] }
            ]
        }"#;
        let draft: IssuesDraft = serde_json::from_str(json).expect("parse loose draft");
        assert!(draft.milestone.is_none());
        assert!(draft.prd_path.is_none());
        assert_eq!(draft.issue_count(), 1);
        assert!(draft.issues[0].blocked_by.is_empty());
    }
}
