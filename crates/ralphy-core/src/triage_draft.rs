//! The agent-triage preview-draft schema (ADR-0017). A judgment agent session
//! reads each open issue carrying `triage-agent` — its body and full comment
//! thread — and emits a [`TriageDraft`] against this Rust-defined schema: a LOCAL
//! preview, never a direct GitHub write. `ralphy triage` then summarizes it, the
//! operator confirms (or `--yes` publishes directly), and only then does the cli
//! apply the verdicts via `gh`. The schema lives in core (beside
//! [`crate::IssuesDraft`]) because it is a domain artifact shared by the agent
//! crates (which produce it) and the cli (which consumes and publishes it).

use serde::{Deserialize, Serialize};

/// One issue's triage verdict (ADR-0017 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriageVerdict {
    /// Executable as-is: swap `triage-agent` for the queue label. No comment.
    Promote,
    /// The executable spec must be assembled from body + thread: post the
    /// consolidated-spec comment, then swap the labels.
    Consolidate,
    /// Under-specified even with the thread: comment what is missing and swap
    /// `triage-agent` for the reporter-bounce label (`needs-info`).
    Bounce,
}

/// One triaged issue: its number, the verdict, and the comment body the verdict
/// requires. `promote` needs no comment; `consolidate` carries the full
/// consolidated-spec body (marker included by the agent); `bounce` carries the
/// what-is-missing note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageItem {
    /// The open issue this verdict applies to.
    pub number: u64,
    /// The triage verdict.
    pub verdict: TriageVerdict,
    /// The comment body: the consolidated spec (`consolidate`) or the
    /// what-is-missing note (`bounce`). Absent/empty for `promote`.
    #[serde(default)]
    pub comment: Option<String>,
}

impl TriageItem {
    /// A verdict is well-formed when the arms that must speak carry a non-empty
    /// comment: `consolidate` (the spec) and `bounce` (the missing-info note).
    /// `promote` must NOT carry one. Returns the offending reason, or `None`.
    fn invalid_reason(&self) -> Option<String> {
        let has_comment = self
            .comment
            .as_deref()
            .map(|c| !c.trim().is_empty())
            .unwrap_or(false);
        match self.verdict {
            TriageVerdict::Consolidate if !has_comment => Some(format!(
                "#{}: consolidate needs a comment body",
                self.number
            )),
            TriageVerdict::Bounce if !has_comment => Some(format!(
                "#{}: bounce needs a what-is-missing comment",
                self.number
            )),
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

    /// Reject a draft whose verdicts are self-contradictory (a `consolidate` or
    /// `bounce` without the comment its arm requires) before any GitHub write.
    /// Returns the first offending reason.
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
            { "number": 12, "verdict": "promote" },
            { "number": 15, "verdict": "consolidate", "comment": "<!-- ralphy:consolidated-spec -->\n## Consolidated spec\n..." },
            { "number": 18, "verdict": "bounce", "comment": "Missing acceptance criteria and a repro." }
        ]
    }"###;

    #[test]
    fn triage_draft_parses_sample_json() {
        let draft: TriageDraft = serde_json::from_str(SAMPLE_JSON).expect("parse sample");
        assert_eq!(draft.item_count(), 3);
        assert_eq!(draft.items[0].verdict, TriageVerdict::Promote);
        assert!(draft.items[0].comment.is_none());
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
                },
                TriageItem {
                    number: 2,
                    verdict: TriageVerdict::Bounce,
                    comment: Some("needs a repro".into()),
                },
            ],
        };
        let json = serde_json::to_string(&draft).expect("serialize");
        let back: TriageDraft = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(draft, back);
    }

    #[test]
    fn unknown_verdict_is_rejected() {
        let json = r#"{ "items": [ { "number": 1, "verdict": "escalate" } ] }"#;
        assert!(serde_json::from_str::<TriageDraft>(json).is_err());
    }

    #[test]
    fn consolidate_without_comment_is_invalid() {
        let draft = TriageDraft {
            items: vec![TriageItem {
                number: 7,
                verdict: TriageVerdict::Consolidate,
                comment: None,
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
            }],
        };
        assert!(draft.validate().is_err(), "whitespace comment is empty");
    }

    #[test]
    fn promote_with_comment_is_still_valid() {
        // A stray comment on promote is tolerated (ignored at apply time), not a
        // validation failure — the agent occasionally over-explains.
        let draft = TriageDraft {
            items: vec![TriageItem {
                number: 3,
                verdict: TriageVerdict::Promote,
                comment: Some("looks good".into()),
            }],
        };
        assert!(draft.validate().is_ok());
    }
}
