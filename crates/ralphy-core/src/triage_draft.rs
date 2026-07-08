//! The agent-triage preview-draft schema (ADR-0017). A judgment agent session
//! reads each open issue carrying `triage-agent` — its body and full comment
//! thread — and emits a [`TriageDraft`] against this Rust-defined schema: a LOCAL
//! preview, never a direct GitHub write. `ralphy triage` then summarizes it, the
//! operator confirms (or `--yes` publishes directly), and only then does the cli
//! apply the verdicts via `gh`. The schema lives in core (beside
//! [`crate::IssuesDraft`]) because it is a domain artifact shared by the agent
//! crates (which produce it) and the cli (which consumes and publishes it).

use serde::{Deserialize, Serialize};

/// One issue's triage verdict (ADR-0017 §2, ADR-0018 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriageVerdict {
    /// Executable as-is: post the evidence-stamp comment (the ADR-0018
    /// evidence-gate citations that justified admitting it), then swap
    /// `triage-agent` for the queue label (ADR-0027). No rewriting of the body.
    Promote,
    /// The executable spec must be assembled from body + thread: post the
    /// consolidated-spec comment, then swap the labels.
    Consolidate,
    /// Under-specified even with the thread: comment what is missing and swap
    /// `triage-agent` for the reporter-bounce label (`needs-info`).
    Bounce,
    /// Accepted, but a *maintainer* owes a decision before any agent works it
    /// (a business-rule or flow change, an ADR-shaped call, or a scope too
    /// large for one executable spec). Post the decision-delivering comment and
    /// swap `triage-agent` for `ready-for-human` (ADR-0018 §3). Human-first, not
    /// reporter-owes-info: it never enters the queue on its own.
    Escalate,
}

/// A follow-up issue an `escalate` verdict may propose (ADR-0018 §4). Machine-
/// readable so the interactive create step can call the tracker directly rather
/// than parsing title/body out of the comment prose. The drafted `body` carries
/// any `Closes #<original>` line itself; issue creation is always human-confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftIssue {
    /// The proposed follow-up issue title.
    pub title: String,
    /// The proposed follow-up issue body (may carry a `Closes #<original>` line).
    pub body: String,
}

/// One triaged issue: its number, the verdict, and the comment body the verdict
/// requires. `promote` carries the evidence stamp (marker included, ADR-0027);
/// `consolidate` carries the full consolidated-spec body (marker included by the
/// agent); `bounce` carries the what-is-missing note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageItem {
    /// The open issue this verdict applies to.
    pub number: u64,
    /// The triage verdict.
    pub verdict: TriageVerdict,
    /// The comment body: the evidence stamp (`promote`), the consolidated spec
    /// (`consolidate`), the what-is-missing note (`bounce`), or the
    /// decision-delivering diagnostic (`escalate`). Required by every verdict
    /// (ADR-0027 made `promote` carry its evidence stamp).
    #[serde(default)]
    pub comment: Option<String>,
    /// An optional follow-up issue an `escalate` verdict proposes (ADR-0018 §4).
    /// `#[serde(default)]` keeps drafts without it backward-compatible; the
    /// interactive create step (never `--yes`) previews and creates it.
    #[serde(default)]
    pub draft_issue: Option<DraftIssue>,
}

impl TriageItem {
    /// A verdict is well-formed when it carries the non-empty comment its arm
    /// requires: `promote` (the evidence stamp, ADR-0027), `consolidate` (the
    /// spec), `bounce` (the missing-info note), and `escalate` (the
    /// decision-delivering diagnostic). Every verdict now speaks. Returns the
    /// offending reason, or `None`.
    fn invalid_reason(&self) -> Option<String> {
        let has_comment = self
            .comment
            .as_deref()
            .map(|c| !c.trim().is_empty())
            .unwrap_or(false);
        match self.verdict {
            TriageVerdict::Promote if !has_comment => Some(format!(
                "#{}: promote needs an evidence-stamp comment",
                self.number
            )),
            TriageVerdict::Consolidate if !has_comment => Some(format!(
                "#{}: consolidate needs a comment body",
                self.number
            )),
            TriageVerdict::Bounce if !has_comment => Some(format!(
                "#{}: bounce needs a what-is-missing comment",
                self.number
            )),
            TriageVerdict::Escalate if !has_comment => {
                Some(format!("#{}: escalate needs a comment body", self.number))
            }
            _ => None,
        }
    }
}

/// The structured output of one triage session: one verdict per triaged issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageDraft {
    /// The triaged issues, one verdict each.
    pub items: Vec<TriageItem>,
}

impl TriageDraft {
    /// Number of issues triaged.
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Reject a draft whose verdicts are self-contradictory (any verdict without
    /// the comment its arm requires — including a `promote` missing its evidence
    /// stamp, ADR-0027) before any GitHub write. Returns the first offending
    /// reason.
    pub fn validate(&self) -> Result<(), String> {
        for item in &self.items {
            if let Some(reason) = item.invalid_reason() {
                return Err(reason);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canonical draft covering all three verdicts, pinning the wire format.
    const SAMPLE_JSON: &str = r###"{
        "items": [
            { "number": 12, "verdict": "promote", "comment": "<!-- ralphy:promote-evidence -->\n## Evidence (AFK)\n- src/foo.rs:42 raises on empty input" },
            { "number": 15, "verdict": "consolidate", "comment": "<!-- ralphy:consolidated-spec -->\n## Consolidated spec\n..." },
            { "number": 18, "verdict": "bounce", "comment": "Missing acceptance criteria and a repro." }
        ]
    }"###;

    #[test]
    fn triage_draft_parses_sample_json() {
        let draft: TriageDraft = serde_json::from_str(SAMPLE_JSON).expect("parse sample");
        assert_eq!(draft.item_count(), 3);
        assert_eq!(draft.items[0].verdict, TriageVerdict::Promote);
        assert!(draft.items[0]
            .comment
            .as_deref()
            .unwrap()
            .contains("ralphy:promote-evidence"));
        assert_eq!(draft.items[1].verdict, TriageVerdict::Consolidate);
        assert!(draft.items[1]
            .comment
            .as_deref()
            .unwrap()
            .contains("ralphy:consolidated-spec"));
        draft.validate().expect("sample is valid");
    }

    #[test]
    fn serde_round_trip() {
        let draft = TriageDraft {
            items: vec![
                TriageItem {
                    number: 1,
                    verdict: TriageVerdict::Promote,
                    comment: None,
                    draft_issue: None,
                },
                TriageItem {
                    number: 2,
                    verdict: TriageVerdict::Bounce,
                    comment: Some("needs a repro".into()),
                    draft_issue: None,
                },
            ],
        };
        let json = serde_json::to_string(&draft).expect("serialize");
        let back: TriageDraft = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(draft, back);
    }

    #[test]
    fn unknown_verdict_is_rejected() {
        // `escalate` is now a real verdict (ADR-0018 §3); use a genuinely-unknown
        // string so this still proves unknown verdicts error.
        let json = r#"{ "items": [ { "number": 1, "verdict": "banish" } ] }"#;
        assert!(serde_json::from_str::<TriageDraft>(json).is_err());
    }

    #[test]
    fn escalate_parses_validates_and_requires_comment() {
        // A genuine escalate item parses, and validates once it carries a comment.
        let json = r#"{ "number": 21, "verdict": "escalate", "comment": "A maintainer must decide the pricing-rule change; see ## Evidence." }"#;
        let item: TriageItem = serde_json::from_str(json).expect("parse escalate");
        assert_eq!(item.verdict, TriageVerdict::Escalate);
        TriageDraft { items: vec![item] }
            .validate()
            .expect("escalate with a comment is valid");

        // An escalate without a comment is rejected, and the reason names the issue.
        let draft = TriageDraft {
            items: vec![TriageItem {
                number: 21,
                verdict: TriageVerdict::Escalate,
                comment: None,
                draft_issue: None,
            }],
        };
        let err = draft.validate().expect_err("escalate needs a comment");
        assert!(err.contains("#21"), "reason names the issue: {err}");
    }

    #[test]
    fn consolidate_without_comment_is_invalid() {
        let draft = TriageDraft {
            items: vec![TriageItem {
                number: 7,
                verdict: TriageVerdict::Consolidate,
                comment: None,
                draft_issue: None,
            }],
        };
        let err = draft.validate().expect_err("consolidate needs a comment");
        assert!(err.contains("#7"), "reason names the issue: {err}");
    }

    #[test]
    fn bounce_without_comment_is_invalid() {
        let draft = TriageDraft {
            items: vec![TriageItem {
                number: 9,
                verdict: TriageVerdict::Bounce,
                comment: Some("  ".into()),
                draft_issue: None,
            }],
        };
        assert!(draft.validate().is_err(), "whitespace comment is empty");
    }

    #[test]
    fn promote_with_evidence_stamp_is_valid() {
        // ADR-0027: promote carries the evidence stamp; a non-empty comment
        // validates.
        let draft = TriageDraft {
            items: vec![TriageItem {
                number: 3,
                verdict: TriageVerdict::Promote,
                comment: Some(format!(
                    "{}\n## Evidence (AFK)\n- foo.rs:1",
                    crate::PROMOTE_EVIDENCE_MARKER
                )),
                draft_issue: None,
            }],
        };
        assert!(draft.validate().is_ok());
    }

    #[test]
    fn promote_without_comment_is_invalid() {
        // ADR-0027 made promote speak: a label flip with no recorded evidence
        // never publishes.
        let draft = TriageDraft {
            items: vec![TriageItem {
                number: 4,
                verdict: TriageVerdict::Promote,
                comment: None,
                draft_issue: None,
            }],
        };
        let err = draft
            .validate()
            .expect_err("promote needs an evidence stamp");
        assert!(err.contains("#4"), "reason names the issue: {err}");
    }
}
