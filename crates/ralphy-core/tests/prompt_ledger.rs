use ralphy_core::acceptance::{apply_ledger, parse_ledger, Verdict};
use ralphy_core::VerdictKind;

/// The `## Acceptance ledger` example embedded in `prompt.plan.md`, parsed.
/// The example's wording is the prompt's business; these tests only need one
/// `[verified]` and one `[review-only]` line that the parser accepts.
fn prompt_example_verdicts() -> Vec<Verdict> {
    let prompt_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/prompts/prompt.plan.md"
    );
    let content =
        std::fs::read_to_string(prompt_path).expect("prompt.plan.md must exist at assets/prompts/");
    parse_ledger(&content)
}

/// The first verdict of `kind` in the example.
fn first(verdicts: &[Verdict], kind: VerdictKind) -> &Verdict {
    verdicts
        .iter()
        .find(|v| v.kind == kind)
        .unwrap_or_else(|| panic!("the prompt ledger example must carry a {kind:?} line"))
}

/// The documented ledger format is exactly what the #12 parser accepts: the
/// example in `prompt.plan.md` parses into typed verdicts.
#[test]
fn prompt_plan_ledger_example_parses_into_typed_verdicts() {
    let verdicts = prompt_example_verdicts();
    let verified = first(&verdicts, VerdictKind::Verified);
    assert!(!verified.criterion.is_empty(), "{verified:?}");
    assert!(
        !verified.evidence.is_empty(),
        "verified verdict must have non-empty evidence text: {verified:?}"
    );
    let review_only = first(&verdicts, VerdictKind::ReviewOnly);
    assert!(!review_only.criterion.is_empty(), "{review_only:?}");
}

/// `apply_ledger` ticks the issue-body line of the example's `[verified]`
/// criterion and leaves its `[review-only]` one open.
#[test]
fn prompt_plan_verified_example_ticks_matching_issue_body_line() {
    let verdicts = prompt_example_verdicts();
    let verified = first(&verdicts, VerdictKind::Verified).criterion.clone();
    let review_only = first(&verdicts, VerdictKind::ReviewOnly).criterion.clone();
    let body = format!("- [ ] {verified}\n- [ ] {review_only}\n");

    let result = apply_ledger(&body, &verdicts);

    assert_eq!(
        result.new_body,
        format!("- [x] {verified}\n- [ ] {review_only}\n"),
        "the verified criterion is ticked, the review-only one stays open"
    );
    assert!(
        result.ticked.contains(&verified),
        "apply_ledger must report the verified criterion as ticked"
    );
    assert!(
        result.unmatched.is_empty(),
        "no verified criteria should be unmatched: {:?}",
        result.unmatched
    );
}
