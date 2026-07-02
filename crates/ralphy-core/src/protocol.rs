//! The deterministic protocol lint over `.ralphy/plan.md` (ADR-0015).
//!
//! When the executor emits `RALPHY_DONE_EXIT`, the runner runs these structural
//! checks over the plan before accepting the self-report. The lint asserts
//! PRESENCE AND SHAPE only — every step ticked, the charter's closing sections
//! written, no planner placeholder left in the acceptance ledger. It never
//! judges the truthfulness of what a section says; that stays with the human at
//! merge. Pure functions over markdown strings — no I/O, no `gh` calls.

use regex::Regex;

use crate::acceptance;

/// One structural check's result: a stable human-readable label and, on
/// failure, a short detail naming what exactly is missing.
#[derive(Debug, Clone)]
pub struct ProtocolCheck {
    pub label: &'static str,
    pub passed: bool,
    /// Shown next to a failed check (e.g. how many steps are unticked).
    pub detail: Option<String>,
}

/// The full lint result: every check, in a stable order, pass or fail.
#[derive(Debug, Clone)]
pub struct ProtocolReport {
    pub checks: Vec<ProtocolCheck>,
}

impl ProtocolReport {
    /// The lint passes only when every check does.
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }

    /// The labels of the failed checks, for log lines.
    pub fn failed_labels(&self) -> Vec<&'static str> {
        self.checks
            .iter()
            .filter(|c| !c.passed)
            .map(|c| c.label)
            .collect()
    }
}

/// Run the structural protocol lint over a plan markdown (ADR-0015):
///   - every `## Steps` checkbox is ticked (`- [x]`, no `- [ ]` left);
///   - `## Handoff` and `## Plan friction` sections present and non-blank;
///   - `## Self-review findings` present when the steps carry a self-review
///     step (the charter forbids ticking that step without the artifact);
///   - no `## Acceptance ledger` line still carrying planner placeholder
///     `evidence:` text (empty, `<angle-bracket template>`, or the planning
///     prompt's literal template phrases).
///
/// Presence and shape only — a present-but-false section passes; catching that
/// is the human reviewer's job, not this lint's.
pub fn lint(plan_md: &str) -> ProtocolReport {
    let steps = section(plan_md, r"(?im)^##\s+Steps\s*$");
    let unticked = steps
        .lines()
        .filter(|l| l.trim_start().starts_with("- [ ]"))
        .count();

    let handoff_present = !section(plan_md, r"(?im)^##\s+Handoff\s*$")
        .trim()
        .is_empty();
    let friction_present = !section(plan_md, r"(?im)^##\s+Plan friction\s*$")
        .trim()
        .is_empty();

    // The self-review artifact is required only when the plan actually carries
    // a self-review step; a plan that (deliberately) omitted the step passes.
    let has_self_review_step = Regex::new(r"(?i)self-review")
        .expect("valid regex")
        .is_match(steps);
    let findings_present = !section(plan_md, r"(?im)^##\s+Self-review findings\s*$")
        .trim()
        .is_empty();

    let placeholders: Vec<String> = acceptance::parse_ledger(plan_md)
        .into_iter()
        .filter(|v| is_placeholder_evidence(&v.evidence))
        .map(|v| v.criterion)
        .collect();

    ProtocolReport {
        checks: vec![
            ProtocolCheck {
                label: "every plan step ticked",
                passed: unticked == 0,
                detail: (unticked > 0).then(|| format!("{unticked} step(s) still `- [ ]`")),
            },
            ProtocolCheck {
                label: "## Handoff present",
                passed: handoff_present,
                detail: (!handoff_present).then(|| "section absent or blank".into()),
            },
            ProtocolCheck {
                label: "## Plan friction present",
                passed: friction_present,
                detail: (!friction_present).then(|| "section absent or blank".into()),
            },
            ProtocolCheck {
                label: "## Self-review findings present when the plan has a self-review step",
                passed: !has_self_review_step || findings_present,
                detail: (has_self_review_step && !findings_present)
                    .then(|| "the plan carries a self-review step but no findings section".into()),
            },
            ProtocolCheck {
                label: "no planner placeholder evidence in the acceptance ledger",
                passed: placeholders.is_empty(),
                detail: (!placeholders.is_empty())
                    .then(|| format!("{} ledger line(s) unfilled", placeholders.len())),
            },
        ],
    }
}

/// Whether a ledger `evidence:` text is still the planner's placeholder rather
/// than the executor's concrete backing: empty, an `<angle-bracket template>`,
/// or the planning prompt's literal template phrases. Shape only — real
/// evidence text is never second-guessed.
fn is_placeholder_evidence(evidence: &str) -> bool {
    let t = evidence.trim();
    if t.is_empty() {
        return true;
    }
    if t.starts_with('<') && t.ends_with('>') {
        return true;
    }
    let lower = t.to_lowercase();
    lower.contains("that will prove it") || lower.contains("how a human confirms")
}

/// Render one `✓/✗ label` line per check, shared by the issue-comment block and
/// the repair brief so the two surfaces never drift.
fn render_checks(report: &ProtocolReport) -> String {
    let mut out = String::from("```\n");
    for c in &report.checks {
        let mark = if c.passed { '\u{2713}' } else { '\u{2717}' };
        match (&c.detail, c.passed) {
            (Some(d), false) => out.push_str(&format!("{mark} {}    {d}\n", c.label)),
            _ => out.push_str(&format!("{mark} {}\n", c.label)),
        }
    }
    out.push_str("```\n");
    out
}

/// Render the lint result block published in the issue's close comment
/// (ADR-0015): one ✓/✗ line per check, plus a loud warning when the issue is
/// being closed with violations after the one repair bounce.
pub fn comment_block(report: &ProtocolReport) -> String {
    let mut out = String::from("## Protocol lint\n\n");
    out.push_str(&render_checks(report));
    if !report.passed() {
        out.push_str(
            "\n\u{26a0} Closed WITH protocol violations — the executor did not repair the \
             structural checks above within the one bounce. The checks assert presence and \
             shape only, so review the plan artifacts and the diff with extra care.\n",
        );
    }
    out
}

/// Render the repair brief the runner drops at `.ralphy/protocol-failure.md`
/// after a lint violation (ADR-0015) — the same hand-back mechanism as
/// `verify-failure.md`. It names exactly which structural checks failed and is
/// explicit that each must be satisfied honestly (finish the work, write the
/// artifact), never by ticking or filler.
pub fn failure_brief(stamp: &str, report: &ProtocolReport) -> String {
    let mut out =
        format!("# Protocol lint failed — completion artifacts required (Ralphy run {stamp})\n\n");
    out.push_str(
        "You emitted `RALPHY_DONE_EXIT`, but the runner's structural lint over \
         `.ralphy/plan.md` found the charter's completion protocol unfinished. The \
         session is handed back ONCE to complete it.\n\nFailed checks (\u{2717}):\n\n",
    );
    out.push_str(&render_checks(report));
    out.push_str(
        "\nFix each failed check HONESTLY, then emit `RALPHY_DONE_EXIT` again:\n\
         - Tick a step `- [x]` ONLY when its work is genuinely done and committed; if \
           work remains, finish it (or split the step and finish the rest) first.\n\
         - Write any missing `## Handoff` / `## Plan friction` / `## Self-review \
           findings` section with real content, as the charter specifies — not filler.\n\
         - Replace planner placeholder `evidence:` text in the `## Acceptance ledger` \
           with the real commit hash, test name, or captured command output backing \
           that criterion.\n\n\
         The lint checks structure only, so completing the protocol satisfies it in \
         minutes. The runner re-runs the SAME checks; a second violation closes the \
         issue with this failure report published for the human reviewer.\n",
    );
    out
}

/// Trimmed section body following the first heading matching `re`.
fn section<'a>(md: &'a str, re: &str) -> &'a str {
    let heading_re = Regex::new(re).expect("valid regex");
    crate::markdown::section_after_heading(md, &heading_re)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plan that satisfies every structural check, self-review step included.
    const CLEAN_PLAN: &str = "\
# Plan for #1

## Steps
- [x] do the thing
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

    #[test]
    fn clean_plan_passes_every_check() {
        let report = lint(CLEAN_PLAN);
        assert!(report.passed(), "failed: {:?}", report.failed_labels());
        assert_eq!(report.checks.len(), 5);
    }

    #[test]
    fn unticked_step_fails() {
        let md = CLEAN_PLAN.replace("- [x] do the thing", "- [ ] do the thing");
        let report = lint(&md);
        assert!(!report.passed());
        assert_eq!(report.failed_labels(), vec!["every plan step ticked"]);
        let check = &report.checks[0];
        assert!(check.detail.as_deref().unwrap().contains("1 step(s)"));
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
                "## Steps\n- [x] do\n\n## Acceptance ledger\n\n- [verified] AC \u{2014} evidence: {placeholder}\n\n## Handoff\n- **Delivered**: x\n\n## Plan friction\n- none\n"
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
    fn absent_ledger_is_vacuously_clean() {
        let md = "## Steps\n- [x] do\n\n## Handoff\n- x\n\n## Plan friction\n- none\n";
        assert!(lint(md).passed());
    }

    #[test]
    fn comment_block_marks_pass_fail_and_warns_on_violation() {
        let clean = comment_block(&lint(CLEAN_PLAN));
        assert!(clean.starts_with("## Protocol lint"));
        assert!(clean.contains("\u{2713} every plan step ticked"));
        assert!(!clean.contains('\u{26a0}'), "no warning on a clean lint");

        let dirty = comment_block(&lint("## Steps\n- [ ] never done\n"));
        assert!(dirty.contains("\u{2717} every plan step ticked"));
        assert!(
            dirty.contains('\u{26a0}'),
            "violation close carries the warning"
        );
        assert!(dirty.contains("presence and shape only"));
    }

    #[test]
    fn failure_brief_names_checks_and_forbids_gaming() {
        let report = lint("## Steps\n- [ ] pending\n");
        let brief = failure_brief("stamp-7", &report);
        assert!(brief.contains("Ralphy run stamp-7"));
        assert!(brief.contains("\u{2717} every plan step ticked"));
        assert!(brief.contains("RALPHY_DONE_EXIT"));
        assert!(brief.contains("HONESTLY"));
        assert!(
            brief.contains("SAME"),
            "must say the runner re-runs the same checks"
        );
    }
}
