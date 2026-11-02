use super::*;

/// A plan that satisfies every structural check, self-review step included.
const CLEAN_PLAN: &str = "\
# Plan for #1

## Steps
- [x] do the thing
- [!] manual scrollback check \u{2014} blocked: headless harness, no terminal window
- [x] Self-review: spawn the reviewer skill
- [x] green-build gate

## Acceptance ledger

- [verified] parser works — evidence: test parse_ledger_parses_fixture (commit abc1234)
- [review-only] screen looks right — evidence: reviewed in PR screenshot

## Self-review findings

0 HIGH, 0 MEDIUM, 0 LOW

## Handoff

- **Delivered**: the thing (abc1234)

## Plan friction

- none
";

/// The #310 shape: steps ticked, `## Handoff` and `## Plan friction`
/// present, but NO `## Acceptance ledger` heading at all.
const PLAN_310: &str = "\
# Plan for #310

## Steps
- [x] step one
- [x] step two

## Handoff

- **Delivered**: x

## Plan friction

- none
";

/// The check that carries `label`, wherever it sits in the report.
fn check<'a>(report: &'a ProtocolReport, label: &str) -> &'a ProtocolCheck {
    report
        .checks
        .iter()
        .find(|c| c.label == label)
        .unwrap_or_else(|| panic!("no check labelled {label:?}"))
}

#[test]
fn clean_plan_passes_every_check() {
    let report = lint(CLEAN_PLAN);
    assert!(report.passed(), "failed: {:?}", report.failed_labels());
    // Control: the checks exist, so "every check passed" is not vacuous.
    assert!(!lint("").passed(), "an empty plan fails the lint");
}

#[test]
fn ledger_presence_check_covers_absent_blank_and_unparsable() {
    // (a) no `## Acceptance ledger` heading at all.
    assert_eq!(
        lint(PLAN_310).failed_labels(),
        vec!["## Acceptance ledger present"]
    );

    // (b) heading present, blank body before the next heading.
    let blank = PLAN_310.replace("## Handoff", "## Acceptance ledger\n\n## Handoff");
    assert_eq!(
        lint(&blank).failed_labels(),
        vec!["## Acceptance ledger present"]
    );

    // (c) heading present, prose but no `- [verified]`/`- [review-only]` line.
    let prose = PLAN_310.replace("## Handoff", "## Acceptance ledger\n\n- none\n\n## Handoff");
    assert_eq!(
        lint(&prose).failed_labels(),
        vec!["## Acceptance ledger present"]
    );

    // Positive control: one review-only verdict line is enough to pass.
    let reviewed = PLAN_310.replace(
        "## Handoff",
        "## Acceptance ledger\n\n- [review-only] screen looks right \u{2014} \
         evidence: reviewed in PR\n\n## Handoff",
    );
    assert!(
        lint(&reviewed).passed(),
        "failed: {:?}",
        lint(&reviewed).failed_labels()
    );
}

#[test]
fn unticked_step_fails() {
    let md = CLEAN_PLAN.replace("- [x] do the thing", "- [ ] do the thing");
    let report = lint(&md);
    assert!(!report.passed());
    assert_eq!(report.failed_labels(), vec!["no plan step left open"]);
    let check = check(&report, "no plan step left open");
    assert!(check.detail.as_deref().unwrap().contains("1 step(s)"));
}

#[test]
fn bare_noticed_step_fails_reasoned_one_passes() {
    // A `- [!]` with no inline reason is a silent tick in disguise.
    let md = CLEAN_PLAN.replace(
        "- [!] manual scrollback check \u{2014} blocked: headless harness, no terminal window",
        "- [!] manual scrollback check",
    );
    let report = lint(&md);
    assert_eq!(
        report.failed_labels(),
        vec!["every `- [!]` noticed step carries its inline reason"]
    );
    assert!(check(
        &report,
        "every `- [!]` noticed step carries its inline reason"
    )
    .detail
    .as_deref()
    .unwrap()
    .contains("1 `- [!]` step(s)"));

    // Both tags and EVERY dash spelling pass — a write path that normalizes
    // the em dash must not cost an attempt. The reason text itself is never
    // judged (shape only).
    for reason in [
        "\u{2014} blocked: no interactive terminal in this harness",
        "\u{2014} noticed: width table says 1, conhost renders 2",
        "-- blocked: docker daemon unreachable",
        "-- Noticed: flaky only on Windows",
        "- blocked: ASCII hyphen, the shape that used to bounce",
        "- noticed: ASCII hyphen, the shape that used to bounce",
        "\u{2013} blocked: en dash, the other normalization",
    ] {
        let ok = CLEAN_PLAN.replace(
            "\u{2014} blocked: headless harness, no terminal window",
            reason,
        );
        assert!(lint(&ok).passed(), "should pass with reason: {reason}");
    }

    // A tag with nothing after it is still bare.
    let empty = CLEAN_PLAN.replace(
        "\u{2014} blocked: headless harness, no terminal window",
        "\u{2014} blocked:",
    );
    assert!(!lint(&empty).passed(), "empty reason must fail");

    // The step's own `- ` bullet must not satisfy the separator now that a
    // single hyphen counts: the tag has to follow a dash of its own.
    let prefix_only = CLEAN_PLAN.replace(
        "manual scrollback check \u{2014} blocked: headless harness, no terminal window",
        "noticed: the bullet dash is not the separator",
    );
    assert!(
        !lint(&prefix_only).passed(),
        "the `- [!]` bullet's own dash must not pass as the reason separator"
    );
}

#[test]
fn missing_handoff_and_friction_fail() {
    let md = "# Plan\n\n## Steps\n- [x] done\n";
    let report = lint(md);
    let failed = report.failed_labels();
    assert!(failed.contains(&"## Handoff present"), "{failed:?}");
    assert!(failed.contains(&"## Plan friction present"), "{failed:?}");
}

#[test]
fn blank_handoff_section_counts_as_absent() {
    let md = "# Plan\n\n## Steps\n- [x] done\n\n## Handoff\n\n\n## Plan friction\n- none\n";
    let report = lint(md);
    assert!(report.failed_labels().contains(&"## Handoff present"));
}

#[test]
fn self_review_step_requires_findings_section() {
    let md = "\
# Plan

## Steps
- [x] do
- [x] Self-review: spawn the reviewer

## Acceptance ledger

- [verified] AC \u{2014} evidence: unit test

## Handoff
- **Delivered**: x

## Plan friction
- none
";
    let report = lint(md);
    assert_eq!(
        report.failed_labels(),
        vec!["## Self-review findings present when the plan has a self-review step"]
    );

    // With the findings section, the same plan passes.
    let fixed = format!("{md}\n## Self-review findings\n\n0 HIGH, 0 MEDIUM, 0 LOW\n");
    assert!(lint(&fixed).passed());
}

#[test]
fn no_self_review_step_needs_no_findings() {
    let md = "\
# Plan

## Steps
- [x] do

## Acceptance ledger

- [verified] AC \u{2014} evidence: unit test

## Handoff
- **Delivered**: x

## Plan friction
- none
";
    assert!(lint(md).passed(), "no self-review step → findings optional");
}

#[test]
fn placeholder_evidence_fails() {
    for placeholder in [
        "<step or test that will prove it>",
        "a test that will prove it later",
        "how a human confirms this in the PR",
        "",
    ] {
        let md = format!(
            "## Steps\n- [x] do\n\n## Acceptance ledger\n\n\
             - [verified] AC \u{2014} evidence: {placeholder}\n\n## Handoff\n\
             - **Delivered**: x\n\n## Plan friction\n- none\n"
        );
        let report = lint(&md);
        assert_eq!(
            report.failed_labels(),
            vec!["no planner placeholder evidence in the acceptance ledger"],
            "placeholder not caught: {placeholder:?}"
        );
    }
}

#[test]
fn real_evidence_passes() {
    assert!(!is_placeholder_evidence(
        "test verify_gate_passes_and_issue_closes (commit abc1234)"
    ));
    assert!(!is_placeholder_evidence(
        "cargo test -p ralphy-core: 74 passed"
    ));
}

#[test]
fn comment_block_marks_pass_fail_and_warns_on_violation() {
    let clean = comment_block(&lint(CLEAN_PLAN));
    assert!(clean.starts_with("## Protocol lint"));
    assert!(clean.contains("\u{2713} no plan step left open"));
    assert!(clean.contains("\u{2713} ## Acceptance ledger present"));
    assert!(!clean.contains('\u{26a0}'), "no warning on a clean lint");

    let dirty = comment_block(&lint("## Steps\n- [ ] never done\n"));
    assert!(dirty.contains("\u{2717} no plan step left open"));
    assert!(
        dirty.contains('\u{26a0}'),
        "violation close carries the warning"
    );
    assert!(dirty.contains("presence and shape only"));

    let ledgerless = comment_block(&lint(PLAN_310));
    assert!(ledgerless.contains("\u{2717} ## Acceptance ledger present"));
}

#[test]
fn failure_brief_names_checks_and_forbids_gaming() {
    let report = lint("## Steps\n- [ ] pending\n");
    let brief = failure_brief("stamp-7", &report, "DONE_TOKEN");
    assert!(brief.contains("Ralphy run stamp-7"));
    assert!(brief.contains("\u{2717} no plan step left open"));
    // The brief quotes the injected completion token, not a hardcoded one.
    assert!(brief.contains("`DONE_TOKEN`"));
}

#[test]
fn failure_brief_tells_the_executor_to_write_the_ledger() {
    let brief = failure_brief("stamp-312", &lint(PLAN_310), "DONE_TOKEN");
    assert!(brief.contains("\u{2717} ## Acceptance ledger present"));
    assert!(brief.contains("one ledger line per issue acceptance criterion"));
}
