use ralphy_core::acceptance::{apply_ledger, parse_ledger};
use ralphy_core::VerdictKind;

/// Canonical criterion strings embedded in prompt.plan.md's ## Acceptance ledger section.
/// These constants are the test contract: if the prompt example changes, update both
/// the prompt and these constants together.
const VERIFIED_CRITERION: &str = "cargo test passes with new test covering parse_ledger";
const REVIEW_ONLY_CRITERION: &str = "a dry-run plan mirrors the issue criteria verbatim";

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
        "prompt.plan.md must contain a ## Acceptance ledger section with at least one example verdict"
    );

    // The [verified] entry must carry the verbatim canonical criterion text.
    let verified: Vec<_> = verdicts
        .iter()
        .filter(|v| v.kind == VerdictKind::Verified)
        .collect();
    assert!(
        !verified.is_empty(),
        "at least one [verified] example must appear in the prompt ledger"
    );
    assert_eq!(
        verified[0].criterion, VERIFIED_CRITERION,
        "verified criterion must match the canonical text from the prompt"
    );
    assert!(
        !verified[0].evidence.is_empty(),
        "verified verdict must have non-empty evidence text"
    );

    // The [review-only] entry must carry the verbatim canonical criterion text.
    let review_only: Vec<_> = verdicts
        .iter()
        .filter(|v| v.kind == VerdictKind::ReviewOnly)
        .collect();
    assert!(
        !review_only.is_empty(),
        "at least one [review-only] example must appear in the prompt ledger"
    );
    assert_eq!(
        review_only[0].criterion, REVIEW_ONLY_CRITERION,
        "review-only criterion must match the canonical text from the prompt"
    );
}

/// Verify that apply_ledger ticks a matching `- [ ] <criterion>` issue-body
/// line for the [verified] example extracted from the prompt.
/// Uses a fixed hardcoded body — not synthesized from parsed output — so the
/// assertion proves that apply_ledger matches the specific canonical criterion text.
#[test]
fn prompt_plan_verified_example_ticks_matching_issue_body_line() {
    let prompt_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../prompt.plan.md");
    let content =
        std::fs::read_to_string(prompt_path).expect("prompt.plan.md must exist at the repo root");

    let verdicts = parse_ledger(&content);

    // Fixed issue body with the known canonical criterion strings — not derived from verdicts.
    let body = format!(
        "- [ ] {}\n- [ ] {}\n",
        VERIFIED_CRITERION, REVIEW_ONLY_CRITERION
    );

    let result = apply_ledger(&body, &verdicts);

    // The verified canonical criterion must be ticked.
    assert!(
        result.ticked.contains(&VERIFIED_CRITERION.to_string()),
        "apply_ledger must tick the verified canonical criterion"
    );
    assert!(
        result
            .new_body
            .contains(&format!("- [x] {}", VERIFIED_CRITERION)),
        "issue body must show '- [x] {}' after apply_ledger",
        VERIFIED_CRITERION
    );

    // The review-only criterion must remain unticked.
    assert!(
        result
            .new_body
            .contains(&format!("- [ ] {}", REVIEW_ONLY_CRITERION)),
        "review-only criterion must remain '- [ ] {}' after apply_ledger",
        REVIEW_ONLY_CRITERION
    );

    assert!(
        result.unmatched.is_empty(),
        "no verified criteria should be unmatched: {:?}",
        result.unmatched
    );
}
