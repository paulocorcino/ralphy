//! Parsing the `## Acceptance ledger` from a plan and applying it to a GitHub
//! issue body. Pure functions over markdown strings — no I/O, no `gh` calls.

use regex::Regex;

/// Whether a ledger verdict was automatically verified or requires human review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerdictKind {
    Verified,
    ReviewOnly,
}

/// One entry in the acceptance ledger: the criterion text (verbatim, for
/// matching against the issue body), the verdict kind, and any evidence string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub criterion: String,
    pub kind: VerdictKind,
    pub evidence: String,
}

/// Parse the `## Acceptance ledger` section of a plan markdown into a list of
/// [`Verdict`]s. Returns an empty list when the section is absent or empty.
///
/// Grammar (one bullet per criterion):
///   `- [verified] <criterion text> — evidence: <text>`
///   `- [review-only] <criterion text> — evidence: <text>`
///
/// The em dash (`—`, U+2014) separates the criterion from the evidence key.
/// Evidence text may be empty.
pub fn parse_ledger(md: &str) -> Vec<Verdict> {
    let heading_re = Regex::new(r"(?im)^##\s+Acceptance ledger\s*$").expect("valid regex");
    let end_re = Regex::new(r"(?m)^##\s+").expect("valid regex");

    let Some(start_m) = heading_re.find(md) else {
        return Vec::new();
    };

    let after = &md[start_m.end()..];
    let end = end_re.find(after).map(|m| m.start()).unwrap_or(after.len());
    let section = &after[..end];

    // Match lines: `- [verified|review-only] <criterion> — evidence: <evidence>`
    // \u{2014} is the em dash (—).
    let line_re = Regex::new(
        r"(?m)^\s*-\s*\[(verified|review-only)\]\s+(.+?)\s*\u{2014}\s*evidence:\s*(.*?)\s*$",
    )
    .expect("valid regex");

    let mut verdicts = Vec::new();
    for cap in line_re.captures_iter(section) {
        let kind = match &cap[1] {
            "verified" => VerdictKind::Verified,
            _ => VerdictKind::ReviewOnly,
        };
        verdicts.push(Verdict {
            criterion: cap[2].trim().to_string(),
            kind,
            evidence: cap[3].trim().to_string(),
        });
    }
    verdicts
}

/// Result of applying a ledger to an issue body.
pub struct TickResult {
    /// The updated issue body with verified criteria ticked.
    pub new_body: String,
    /// Criterion texts that were found and ticked (`- [ ]` → `- [x]`).
    pub ticked: Vec<String>,
    /// Verified criteria whose verbatim text was not found as an unchecked line.
    pub unmatched: Vec<String>,
}

/// Apply verified verdicts to an issue body by flipping `- [ ] <criterion>` to
/// `- [x] <criterion>` on exact trimmed-line match. Review-only verdicts are
/// never ticked. Already-ticked lines are left untouched.
pub fn apply_ledger(body: &str, verdicts: &[Verdict]) -> TickResult {
    let mut body_lines: Vec<String> = body.lines().map(str::to_string).collect();
    let had_trailing_newline = body.ends_with('\n');
    let mut ticked = Vec::new();
    let mut unmatched = Vec::new();

    for verdict in verdicts {
        if verdict.kind != VerdictKind::Verified {
            continue;
        }
        let target = format!("- [ ] {}", verdict.criterion);
        let mut found = false;
        for line in body_lines.iter_mut() {
            if !found && line.trim() == target.as_str() {
                *line = line.replacen("- [ ]", "- [x]", 1);
                found = true;
            }
        }
        if found {
            ticked.push(verdict.criterion.clone());
        } else {
            unmatched.push(verdict.criterion.clone());
        }
    }

    let mut new_body = body_lines.join("\n");
    if had_trailing_newline {
        new_body.push('\n');
    }

    TickResult {
        new_body,
        ticked,
        unmatched,
    }
}

/// Render a structured evidence comment pairing each criterion with its verdict
/// and evidence. Review-only and unmatched verified criteria are explicitly
/// flagged for the human reviewer.
pub fn evidence_comment(verdicts: &[Verdict], unmatched: &[String]) -> String {
    let mut out = String::from("## Acceptance ledger\n\n");

    for verdict in verdicts {
        let tag = match verdict.kind {
            VerdictKind::Verified => "verified",
            VerdictKind::ReviewOnly => "review-only",
        };
        let evidence = if verdict.evidence.is_empty() {
            "(no evidence recorded)"
        } else {
            &verdict.evidence
        };

        if unmatched.contains(&verdict.criterion) {
            out.push_str(&format!(
                "- **[{tag}]** {}  \n  Evidence: {}  \n  [NEEDS REVIEW: criterion not found in issue body — not auto-ticked]\n",
                verdict.criterion, evidence
            ));
        } else if verdict.kind == VerdictKind::ReviewOnly {
            out.push_str(&format!(
                "- **[{tag}]** {}  \n  Evidence: {}  \n  [NEEDS REVIEW: review-only — not auto-ticked]\n",
                verdict.criterion, evidence
            ));
        } else {
            out.push_str(&format!(
                "- **[{tag}]** {}  \n  Evidence: {}\n",
                verdict.criterion, evidence
            ));
        }
    }

    if !unmatched.is_empty() {
        out.push_str("\n### Unmatched criteria\n\n");
        out.push_str(
            "The following verified criteria could not be ticked automatically \
             because their verbatim text was not found as an unchecked line in the issue body:\n\n",
        );
        for c in unmatched {
            out.push_str(&format!("- {c}\n"));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEDGER_FIXTURE: &str = "\
# Plan for #1

## Steps
- [ ] do a thing
- [x] done already

## Acceptance ledger

- [verified] Parser returns typed verdicts — evidence: unit tests in acceptance.rs cover parse_ledger
- [review-only] Output looks good to user — evidence: manual spot-check
- [verified] Empty ledger is a no-op — evidence: separate unit test

## Notes
some note
";

    #[test]
    fn parse_ledger_parses_fixture_into_verdicts() {
        let verdicts = parse_ledger(LEDGER_FIXTURE);
        assert_eq!(verdicts.len(), 3);

        assert_eq!(verdicts[0].criterion, "Parser returns typed verdicts");
        assert_eq!(verdicts[0].kind, VerdictKind::Verified);
        assert_eq!(
            verdicts[0].evidence,
            "unit tests in acceptance.rs cover parse_ledger"
        );

        assert_eq!(verdicts[1].criterion, "Output looks good to user");
        assert_eq!(verdicts[1].kind, VerdictKind::ReviewOnly);
        assert_eq!(verdicts[1].evidence, "manual spot-check");

        assert_eq!(verdicts[2].criterion, "Empty ledger is a no-op");
        assert_eq!(verdicts[2].kind, VerdictKind::Verified);
        assert_eq!(verdicts[2].evidence, "separate unit test");
    }

    #[test]
    fn parse_ledger_absent_section_returns_empty() {
        assert!(parse_ledger("# Plan\n\n## Steps\n- [ ] do\n").is_empty());
    }

    #[test]
    fn parse_ledger_empty_section_returns_empty() {
        let md = "## Acceptance ledger\n\n## Notes\nsome note\n";
        assert!(parse_ledger(md).is_empty());
    }

    #[test]
    fn parse_ledger_stops_at_next_heading() {
        let md = "## Acceptance ledger\n\
                  - [verified] Only this one — evidence: yes\n\
                  ## Other\n\
                  - [verified] Not this — evidence: no\n";
        let verdicts = parse_ledger(md);
        assert_eq!(verdicts.len(), 1);
        assert_eq!(verdicts[0].criterion, "Only this one");
    }

    #[test]
    fn apply_ledger_ticks_verified_and_leaves_review_only_untouched() {
        let body = "## Acceptance criteria\n\
                    - [ ] Parser returns typed verdicts\n\
                    - [ ] Output looks good to user\n\
                    - [ ] Empty ledger is a no-op\n";

        let verdicts = parse_ledger(LEDGER_FIXTURE);
        let result = apply_ledger(body, &verdicts);

        // Verified criteria whose text matches are ticked.
        assert!(result
            .new_body
            .contains("- [x] Parser returns typed verdicts"));
        assert!(result.new_body.contains("- [x] Empty ledger is a no-op"));
        // Review-only criteria are never ticked.
        assert!(result.new_body.contains("- [ ] Output looks good to user"));

        assert_eq!(
            result.ticked,
            vec!["Parser returns typed verdicts", "Empty ledger is a no-op"]
        );
        assert!(result.unmatched.is_empty());
    }

    #[test]
    fn apply_ledger_leaves_already_ticked_lines_untouched() {
        let body = "- [x] Parser returns typed verdicts\n- [ ] Empty ledger is a no-op\n";
        let verdicts = vec![
            Verdict {
                criterion: "Parser returns typed verdicts".into(),
                kind: VerdictKind::Verified,
                evidence: "done".into(),
            },
            Verdict {
                criterion: "Empty ledger is a no-op".into(),
                kind: VerdictKind::Verified,
                evidence: "done".into(),
            },
        ];
        let result = apply_ledger(body, &verdicts);
        // Already-ticked line stays ticked; the `- [x]` form does not match `- [ ]`.
        assert!(result
            .new_body
            .contains("- [x] Parser returns typed verdicts"));
        // The already-ticked one is unmatched because `- [ ] Parser…` was not found.
        assert!(result
            .unmatched
            .contains(&"Parser returns typed verdicts".to_string()));
        // The second one is ticked.
        assert!(result.new_body.contains("- [x] Empty ledger is a no-op"));
    }

    #[test]
    fn apply_ledger_reports_unmatched_verified_criterion() {
        let body = "- [ ] Some other criterion\n";
        let verdicts = vec![Verdict {
            criterion: "Not in body".into(),
            kind: VerdictKind::Verified,
            evidence: "irrelevant".into(),
        }];
        let result = apply_ledger(body, &verdicts);
        assert_eq!(result.unmatched, vec!["Not in body"]);
        assert!(result.ticked.is_empty());
        // Body unchanged.
        assert_eq!(result.new_body, body);
    }

    #[test]
    fn apply_ledger_review_only_is_never_ticked() {
        let body = "- [ ] Output looks good to user\n";
        let verdicts = vec![Verdict {
            criterion: "Output looks good to user".into(),
            kind: VerdictKind::ReviewOnly,
            evidence: "manual".into(),
        }];
        let result = apply_ledger(body, &verdicts);
        assert!(result.ticked.is_empty());
        assert!(result.unmatched.is_empty());
        // Body unchanged (review-only, not an error).
        assert_eq!(result.new_body, body);
    }

    #[test]
    fn evidence_comment_flags_review_only_and_unmatched() {
        let verdicts = vec![
            Verdict {
                criterion: "Verified and matched".into(),
                kind: VerdictKind::Verified,
                evidence: "test passes".into(),
            },
            Verdict {
                criterion: "Review only".into(),
                kind: VerdictKind::ReviewOnly,
                evidence: "manual check".into(),
            },
            Verdict {
                criterion: "Verified but missing".into(),
                kind: VerdictKind::Verified,
                evidence: "test passes".into(),
            },
        ];
        let unmatched = vec!["Verified but missing".to_string()];
        let comment = evidence_comment(&verdicts, &unmatched);

        assert!(comment.contains("[verified]") || comment.contains("verified"));
        assert!(
            comment.contains("NEEDS REVIEW"),
            "review-only must be flagged"
        );
        assert!(
            comment.contains("Verified but missing"),
            "unmatched must appear"
        );
        assert!(
            comment.contains("Unmatched criteria"),
            "unmatched section present"
        );
        // Matched verified criterion must NOT be flagged.
        assert!(
            !comment.contains("Verified and matched\n  Evidence: test passes  \n  [NEEDS REVIEW")
        );
    }
}
