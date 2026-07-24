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
    let section = crate::markdown::section_after_heading(md, &heading_re);

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

/// Normalize an acceptance line for matching only: drop the inline-markdown
/// delimiters (`*`, `_`, and backtick) that a ledger criterion routinely loses
/// when it is transcribed from the issue's AC bullet, collapse runs of
/// whitespace, and drop trailing sentence punctuation (`.`, `;`, `:`, `,`) that
/// the transcription just as routinely drops when it rewrites the AC bullet as a
/// bare clause. Applied to BOTH sides of the comparison, so an identifier like
/// `blob_id` reduces identically on each side and still matches — this affects
/// matching alone; the ticked line keeps its original text verbatim.
fn normalize_ac(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '*' | '_' | '`'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(['.', ';', ':', ','])
        .to_string()
}

/// Collect the checkbox list items of a body as *logical* bullets: each item
/// is the physical line index where its `- [` bullet starts, paired with the
/// bullet's full text — soft-wrapped continuation lines joined by a single
/// space. Issue bodies hard-wrap their AC prose at ~78 columns, so a criterion
/// routinely spans several physical lines; matching against the physical line
/// alone can never tick those (the #266 failure). A continuation line is a
/// non-empty indented line that does not itself start a new bullet or heading;
/// a blank line ends the item.
fn logical_checkbox_items(lines: &[String]) -> Vec<(usize, String)> {
    let mut items = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim_start().starts_with("- [") {
            let mut text = lines[i].clone();
            let mut j = i + 1;
            while j < lines.len() {
                let trimmed = lines[j].trim_start();
                let continues = !trimmed.is_empty()
                    && lines[j].starts_with(char::is_whitespace)
                    && !trimmed.starts_with('-')
                    && !trimmed.starts_with('#');
                if !continues {
                    break;
                }
                text.push(' ');
                text.push_str(trimmed);
                j += 1;
            }
            items.push((i, text));
            i = j;
        } else {
            i += 1;
        }
    }
    items
}

/// Apply verified verdicts to an issue body by flipping `- [ ] <criterion>` to
/// `- [x] <criterion>`. Matching is verbatim *modulo inline markdown,
/// whitespace and soft wrapping* (see [`normalize_ac`] and
/// [`logical_checkbox_items`]) — the ledger criterion is frequently transcribed
/// without the issue line's `**bold**`/`` `code` `` markers and as one logical
/// line where the issue hard-wraps, and an exact per-line match would silently
/// drop those ticks. Review-only verdicts are never ticked. Already-ticked
/// lines are left untouched.
pub fn apply_ledger(body: &str, verdicts: &[Verdict]) -> TickResult {
    let mut body_lines: Vec<String> = body.lines().map(str::to_string).collect();
    let had_trailing_newline = body.ends_with('\n');
    let mut ticked = Vec::new();
    let mut unmatched = Vec::new();

    for verdict in verdicts {
        if verdict.kind != VerdictKind::Verified {
            continue;
        }
        let target = normalize_ac(&format!("- [ ] {}", verdict.criterion));
        // Recomputed per verdict so a bullet ticked by an earlier verdict no
        // longer matches (its `[x]` normalizes differently), letting duplicate
        // criteria tick successive occurrences exactly as before.
        let mut found = false;
        for (start, text) in logical_checkbox_items(&body_lines) {
            // Compare normalized logical bullets, but flip the box on the
            // original first line so its markdown and wrapping survive.
            // Replacing `[ ]` (not `- [ ]`) tolerates bullet/whitespace
            // variants the normalized match also accepts.
            if normalize_ac(&text) == target {
                body_lines[start] = body_lines[start].replacen("[ ]", "[x]", 1);
                found = true;
                break;
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
    fn apply_ledger_ticks_through_inline_markdown_mismatch() {
        // The #10 failure: the issue's AC line carries inline `**bold**`/`` `code` ``
        // that the ledger criterion dropped when it was transcribed. A verbatim
        // match left these verified criteria unticked; normalized matching ticks
        // them while preserving the line's original markdown.
        let body = "## Acceptance criteria\n\
            - [ ] Teste **\"Wallet A não busca blob de B\"**: posse do `blob_id` busca; sem ela, nega — provado contra um Supabase real\n\
            - [ ] Rate-limit anti-abuso por sessão demonstrado **sem** linha `auth↔blob` persistida\n";
        let verdicts = vec![
            Verdict {
                // criterion as the ledger recorded it: bold + backticks stripped.
                criterion:
                    "Teste \"Wallet A não busca blob de B\": posse do blob_id busca; sem ela, nega — provado contra um Supabase real"
                        .into(),
                kind: VerdictKind::Verified,
                evidence: "proof.sh".into(),
            },
            Verdict {
                criterion: "Rate-limit anti-abuso por sessão demonstrado sem linha auth↔blob persistida"
                    .into(),
                kind: VerdictKind::Verified,
                evidence: "capability.test.mjs".into(),
            },
        ];
        let result = apply_ledger(body, &verdicts);
        assert!(
            result.unmatched.is_empty(),
            "inline markdown must not block matching: {:?}",
            result.unmatched
        );
        assert_eq!(result.ticked.len(), 2, "both verified criteria tick");
        // The ticked lines keep their original markdown — only the box flips.
        assert!(result.new_body.contains(
            "- [x] Teste **\"Wallet A não busca blob de B\"**: posse do `blob_id` busca"
        ));
        assert!(result
            .new_body
            .contains("- [x] Rate-limit anti-abuso por sessão demonstrado **sem** linha `auth↔blob` persistida"));
    }

    #[test]
    fn apply_ledger_ticks_through_trailing_punctuation_mismatch() {
        // The #152/#153 failure: the issue's AC bullets are written as full
        // sentences ending in a period, but the ledger transcribed each criterion
        // as a bare clause without the trailing `.`. Inline-markdown normalization
        // alone left them unmatched → every verified criterion was flagged
        // NEEDS REVIEW instead of auto-ticked. Trailing-punctuation trimming ticks
        // them while preserving the line's original text.
        let body = "## Acceptance criteria\n\
            - [ ] Usage is summed per-step across `TurnBegin`/`TurnEnd`, not double-counted.\n\
            - [ ] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` pass on Windows and Linux.\n";
        let verdicts = vec![
            Verdict {
                // criterion as the ledger recorded it: backticks and the trailing
                // period both stripped.
                criterion:
                    "Usage is summed per-step across TurnBegin/TurnEnd, not double-counted".into(),
                kind: VerdictKind::Verified,
                evidence: "unit test".into(),
            },
            Verdict {
                criterion:
                    "cargo fmt --check, cargo clippy -- -D warnings, cargo test pass on Windows and Linux"
                        .into(),
                kind: VerdictKind::Verified,
                evidence: "CI".into(),
            },
        ];
        let result = apply_ledger(body, &verdicts);
        assert!(
            result.unmatched.is_empty(),
            "trailing punctuation must not block matching: {:?}",
            result.unmatched
        );
        assert_eq!(result.ticked.len(), 2, "both verified criteria tick");
        // The ticked lines keep their original text, period and all — only the box flips.
        assert!(result.new_body.contains(
            "- [x] Usage is summed per-step across `TurnBegin`/`TurnEnd`, not double-counted."
        ));
        assert!(result.new_body.contains(
            "- [x] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` pass on Windows and Linux."
        ));
    }

    #[test]
    fn apply_ledger_ticks_through_soft_wrapped_bullets() {
        // The #266 failure: the issue's AC bullets are hard-wrapped at ~78
        // columns with indented continuation lines, but the ledger transcribed
        // each criterion as one logical line. Per-physical-line matching left
        // every wrapped criterion flagged NEEDS REVIEW while the one
        // single-line criterion ticked. Logical-bullet matching ticks them and
        // preserves the original wrapping.
        // Built with join so the continuation lines keep their literal
        // indentation (a `\n\` string continuation would strip it).
        let body = [
            "## Acceptance criteria",
            "",
            "- [ ] A quota failure stops the run and is reported as a limit, not as a crash",
            "      and not as a mute stall — the operator sees the vendor's own sentence",
            "- [ ] Ralphy performs no retry of its own on a quota failure",
            "- [ ] A quota stop reached **after** the session committed real work is reported",
            "      as such: the partial work is named, not silently discarded, and the branch",
            "      is handed back for inspection",
            "",
        ]
        .join("\n");
        let verdicts = vec![
            Verdict {
                criterion: "A quota failure stops the run and is reported as a limit, not as a crash and not as a mute stall — the operator sees the vendor's own sentence".into(),
                kind: VerdictKind::Verified,
                evidence: "fixture test".into(),
            },
            Verdict {
                criterion: "Ralphy performs no retry of its own on a quota failure".into(),
                kind: VerdictKind::Verified,
                evidence: "pin test".into(),
            },
            Verdict {
                // Bold stripped in transcription AND wrapped across three lines.
                criterion: "A quota stop reached after the session committed real work is reported as such: the partial work is named, not silently discarded, and the branch is handed back for inspection".into(),
                kind: VerdictKind::Verified,
                evidence: "midturn fixture test".into(),
            },
        ];
        let result = apply_ledger(&body, &verdicts);
        assert!(
            result.unmatched.is_empty(),
            "soft wrapping must not block matching: {:?}",
            result.unmatched
        );
        assert_eq!(result.ticked.len(), 3, "all verified criteria tick");
        // Only the box on the first physical line flips; the wrapped
        // continuation lines are untouched.
        let wrapped_first = [
            "- [x] A quota failure stops the run and is reported as a limit, not as a crash",
            "      and not as a mute stall — the operator sees the vendor's own sentence",
        ]
        .join("\n");
        assert!(result.new_body.contains(&wrapped_first));
        let wrapped_third = [
            "- [x] A quota stop reached **after** the session committed real work is reported",
            "      as such: the partial work is named, not silently discarded, and the branch",
            "      is handed back for inspection",
        ]
        .join("\n");
        assert!(result.new_body.contains(&wrapped_third));
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

        // The exact tag format must appear for verified entries.
        assert!(
            comment.contains("**[verified]**"),
            "verified tag must appear in bold-bracket form"
        );
        // Review-only and unmatched entries must be flagged for human review.
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
        // The matched verified criterion must NOT carry a NEEDS REVIEW flag.
        // Split on the criterion text and check only the line that follows it.
        if let Some(idx) = comment.find("Verified and matched") {
            let tail = &comment[idx..];
            let line_end = tail.find('\n').unwrap_or(tail.len());
            // The criterion line plus the next evidence line must not contain the flag.
            let two_lines_end = tail[line_end + 1..]
                .find('\n')
                .map(|n| line_end + 1 + n)
                .unwrap_or(tail.len());
            assert!(
                !tail[..two_lines_end].contains("[NEEDS REVIEW"),
                "matched verified criterion must not be flagged: {}",
                &tail[..two_lines_end]
            );
        }
    }
}
