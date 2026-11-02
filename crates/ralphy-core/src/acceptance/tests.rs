use super::*;

const LEDGER_FIXTURE: &str = "\
# Plan for #1

## Steps
- [ ] do a thing
- [x] done already

## Acceptance ledger

- [verified] Parser returns typed verdicts — evidence: unit tests in acceptance.rs cover \
parse_ledger
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
fn parse_ledger_accepts_every_dash_spelling() {
    // An em dash normalized on the way to disk used to drop the line from
    // the ledger silently; the protocol lint then blamed a missing section.
    let md = "## Acceptance ledger\n\
              - [verified] em dash \u{2014} evidence: a\n\
              - [verified] en dash \u{2013} evidence: b\n\
              - [verified] double hyphen -- evidence: c\n\
              - [verified] single hyphen - evidence: d\n";
    let verdicts = parse_ledger(md);
    assert_eq!(verdicts.len(), 4);
    for (v, expected) in verdicts.iter().zip(["a", "b", "c", "d"]) {
        assert_eq!(v.evidence, expected);
        assert_eq!(v.kind, VerdictKind::Verified);
    }

    // A hyphen inside the criterion is not the separator — only the one
    // immediately preceding `evidence:` is.
    let hyphenated = parse_ledger(
        "## Acceptance ledger\n- [verified] auto-pan reaches the far axis - evidence: e\n",
    );
    assert_eq!(hyphenated.len(), 1);
    assert_eq!(hyphenated[0].criterion, "auto-pan reaches the far axis");
    assert_eq!(hyphenated[0].evidence, "e");
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
        - [ ] Teste **\"Wallet A não busca blob de B\"**: posse do `blob_id` busca; \
        sem ela, nega — provado contra um Supabase real\n\
        - [ ] Rate-limit anti-abuso por sessão demonstrado **sem** linha `auth↔blob` persistida\n";
    let verdicts = vec![
        Verdict {
            // criterion as the ledger recorded it: bold + backticks stripped.
            criterion:
                "Teste \"Wallet A não busca blob de B\": posse do blob_id busca; sem ela, nega — \
                 provado contra um Supabase real"
                    .into(),
            kind: VerdictKind::Verified,
            evidence: "proof.sh".into(),
        },
        Verdict {
            criterion: "Rate-limit anti-abuso por sessão demonstrado sem linha \
                 auth↔blob persistida"
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
    assert!(result
        .new_body
        .contains("- [x] Teste **\"Wallet A não busca blob de B\"**: posse do `blob_id` busca"));
    assert!(result.new_body.contains(
        "- [x] Rate-limit anti-abuso por sessão demonstrado \
             **sem** linha `auth↔blob` persistida"
    ));
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
        - [ ] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` pass on \
        Windows and Linux.\n";
    let verdicts = vec![
        Verdict {
            // criterion as the ledger recorded it: backticks and the trailing
            // period both stripped.
            criterion: "Usage is summed per-step across TurnBegin/TurnEnd, not double-counted"
                .into(),
            kind: VerdictKind::Verified,
            evidence: "unit test".into(),
        },
        Verdict {
            criterion: "cargo fmt --check, cargo clippy -- -D warnings, cargo test pass on \
                 Windows and Linux"
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
        "- [x] `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` pass on \
         Windows and Linux."
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
            criterion:
                "A quota failure stops the run and is reported as a limit, not as a crash and \
             not as a mute stall — the operator sees the vendor's own sentence"
                    .into(),
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
            criterion: "A quota stop reached after the session committed real work is reported as \
             such: the partial work is named, not silently discarded, and the branch \
             is handed back for inspection"
                .into(),
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
fn evidence_comment_names_a_verdict_with_no_evidence() {
    let verdicts = vec![Verdict {
        criterion: "Bare verdict".into(),
        kind: VerdictKind::Verified,
        evidence: String::new(),
    }];
    let comment = evidence_comment(&verdicts, &[]);
    assert!(
        comment.contains("- **[verified]** Bare verdict  \n  Evidence: (no evidence recorded)\n"),
        "{comment}"
    );
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
