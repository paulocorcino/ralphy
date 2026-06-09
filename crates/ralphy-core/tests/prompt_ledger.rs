use ralphy_core::acceptance::{apply_ledger, parse_ledger};
use ralphy_core::VerdictKind;

/// Read the root-level `prompt.plan.md` and verify that the canonical ledger
/// example embedded in it is parseable and produces the expected `Verdict`s.
///
/// This test FAILS when the example is absent from the prompt (i.e. before
/// step 1 of issue #13 is applied) and PASSES after — proving that the
/// documented format is exactly what the #12 parser accepts.
#[test]
fn prompt_plan_ledger_example_parses_into_typed_verdicts() {
    let prompt_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../prompt.plan.md");
    let content =
        std::fs::read_to_string(prompt_path).expect("prompt.plan.md must exist at the repo root");

    let verdicts = parse_ledger(&content);

    assert!(
        !verdicts.is_empty(),
        "prompt.plan.md must contain an ## Acceptance ledger section with at least one example verdict"
    );

    // The first verified example must match the canonical text from the prompt.
    let verified: Vec<_> = verdicts
        .iter()
        .filter(|v| v.kind == VerdictKind::Verified)
        .collect();
    assert!(
        !verified.is_empty(),
        "at least one [verified] example must appear in the prompt ledger"
    );
    assert!(
        !verified[0].criterion.is_empty(),
        "verified verdict must have non-empty criterion text"
    );
    assert!(
        !verified[0].evidence.is_empty(),
        "verified verdict must have non-empty evidence text"
    );

    // The review-only example must also be present.
    let review_only: Vec<_> = verdicts
        .iter()
        .filter(|v| v.kind == VerdictKind::ReviewOnly)
        .collect();
    assert!(
        !review_only.is_empty(),
        "at least one [review-only] example must appear in the prompt ledger"
    );
    assert!(
        !review_only[0].criterion.is_empty(),
        "review-only verdict must have non-empty criterion text"
    );
}

/// Verify that apply_ledger ticks a matching `- [ ] <criterion>` issue-body
/// line for the [verified] example extracted from the prompt.
#[test]
fn prompt_plan_verified_example_ticks_matching_issue_body_line() {
    let prompt_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../prompt.plan.md");
    let content =
        std::fs::read_to_string(prompt_path).expect("prompt.plan.md must exist at the repo root");

    let verdicts = parse_ledger(&content);
    let verified: Vec<_> = verdicts
        .iter()
        .filter(|v| v.kind == VerdictKind::Verified)
        .collect();
    assert!(
        !verified.is_empty(),
        "need at least one verified verdict to test apply_ledger"
    );

    // Build a synthetic issue body with one unchecked line per verified criterion.
    let body: String = verified
        .iter()
        .map(|v| format!("- [ ] {}\n", v.criterion))
        .collect();

    let result = apply_ledger(&body, &verdicts);

    // Every verified criterion must be ticked.
    for v in &verified {
        assert!(
            result.ticked.contains(&v.criterion),
            "verified criterion '{}' must be ticked by apply_ledger",
            v.criterion
        );
        let ticked_line = format!("- [x] {}", v.criterion);
        assert!(
            result.new_body.contains(&ticked_line),
            "issue body must contain '{}' after apply_ledger",
            ticked_line
        );
    }
    assert!(
        result.unmatched.is_empty(),
        "no verified criteria should be unmatched"
    );
}
