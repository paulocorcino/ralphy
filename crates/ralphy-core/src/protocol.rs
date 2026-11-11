//! The deterministic protocol lint over `.ralphy/plan.md` (ADR-0015).
//!
//! When the executor signals completion (the adapter-defined done sentinel),
//! the runner runs these structural checks over the plan before accepting the
//! self-report. The lint asserts
//! PRESENCE AND SHAPE only — no step left open, the charter's closing sections
//! written, the acceptance ledger present and parsed, no planner placeholder
//! left in it. It never judges the truthfulness of what a section says; that
//! stays with the human at merge. Pure functions over markdown strings — no
//! I/O, no `gh` calls.

use regex::Regex;
use std::fmt::Write as _;

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
///   - every `## Steps` checkbox is resolved — no `- [ ]` left (`- [x]`
///     checked and `- [!]` noticed both count as resolved);
///   - every `- [!]` noticed step carries its reason inline on the step line
///     (`— blocked: <text>` or `— noticed: <text>`, any dash spelling) — a bare
///     `- [!]` would be a silent tick in disguise;
///   - `## Handoff` and `## Plan friction` sections present and non-blank;
///   - `## Acceptance ledger` present and parses to at least one verdict line
///     (#312) — an absent, blank, or prose-only section fails this check;
///     presence only, not truthfulness;
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

    // A noticed step resolves the open-step check, but only with its reason
    // inline on the same line — the attempted verification's literal blocker
    // (or the surprise). Shape only: any non-empty text after the tag passes.
    // Every dash spelling is accepted: an executor whose write path normalizes
    // U+2014 to `-` or `–` was losing a whole attempt to the glyph, not the
    // reason, and the reason is the only thing this check is about.
    let reason_re = Regex::new(r"(?i)(?:\u{2014}|\u{2013}|-{1,2})\s*(?:blocked|noticed):\s*\S")
        .expect("valid regex");
    let bare_noticed = steps
        .lines()
        .filter(|l| l.trim_start().starts_with("- [!]"))
        .filter(|l| !reason_re.is_match(l))
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

    let verdicts = acceptance::parse_ledger(plan_md);
    let ledger_present = !verdicts.is_empty();
    let placeholders: Vec<String> = verdicts
        .into_iter()
        .filter(|v| is_placeholder_evidence(&v.evidence))
        .map(|v| v.criterion)
        .collect();

    ProtocolReport {
        checks: vec![
            ProtocolCheck {
                label: "no plan step left open",
                passed: unticked == 0,
                detail: (unticked > 0).then(|| format!("{unticked} step(s) still `- [ ]`")),
            },
            ProtocolCheck {
                label: "every `- [!]` noticed step carries its inline reason",
                passed: bare_noticed == 0,
                detail: (bare_noticed > 0).then(|| {
                    format!(
                        "{bare_noticed} `- [!]` step(s) without an inline \
                         `\u{2014} blocked:`/`\u{2014} noticed:` reason"
                    )
                }),
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
                label: "## Acceptance ledger present",
                passed: ledger_present,
                detail: (!ledger_present).then(|| {
                    if section(plan_md, r"(?im)^##\s+Acceptance ledger\s*$")
                        .trim()
                        .is_empty()
                    {
                        "section absent or blank".to_owned()
                    } else {
                        "section present but no `- [verified]`/`- [review-only]` line parsed"
                            .to_owned()
                    }
                }),
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
            (Some(d), false) => {
                writeln!(out, "{mark} {}    {d}", c.label).expect(crate::WRITE_TO_STRING);
            }
            _ => writeln!(out, "{mark} {}", c.label).expect(crate::WRITE_TO_STRING),
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
/// artifact), never by ticking or filler. `done_signal` is the completion token
/// the active adapter's charter defines — received as data so the brief speaks
/// the agent's own protocol without the core knowing the literal (ADR-0002).
pub fn failure_brief(stamp: &str, report: &ProtocolReport, done_signal: &str) -> String {
    let mut out =
        format!("# Protocol lint failed — completion artifacts required (Ralphy run {stamp})\n\n");
    writeln!(
        out,
        "You emitted `{done_signal}`, but the runner's structural lint over \
         `.ralphy/plan.md` found the charter's completion protocol unfinished. The \
         session is handed back ONCE to complete it.\n\nFailed checks (\u{2717}):\n",
    )
    .expect(crate::WRITE_TO_STRING);
    out.push_str(&render_checks(report));
    writeln!(
        out,
        "\nFix each failed check HONESTLY, then emit `{done_signal}` again:\n\
         - Tick a step `- [x]` ONLY when its work is genuinely done and committed; if \
           work remains, finish it (or split the step and finish the rest) first.\n\
         - A `- [!]` noticed step must end with its reason on the SAME line \
           (`-- blocked: <the literal error>` or `-- noticed: <the surprise>`; an em \
           dash works too), backed by an attempt recorded under `## Notes & decisions` \
           \u{2014} never use `- [!]` to dodge work a command could still verify.\n\
         - Write any missing `## Handoff` / `## Plan friction` / `## Self-review \
           findings` section with real content, as the charter specifies — not filler.\n\
         - A missing `## Acceptance ledger` must be WRITTEN: one ledger line per \
           issue acceptance criterion, in the `- [verified|review-only] <criterion> \
           -- evidence: <backing>` shape (an em dash works too).\n\
         - Replace planner placeholder `evidence:` text in the `## Acceptance ledger` \
           with the real commit hash, test name, or captured command output backing \
           that criterion.\n\n\
         The lint checks structure only, so completing the protocol satisfies it in \
         minutes. The runner re-runs the SAME checks; a second violation closes the \
         issue with this failure report published for the human reviewer.",
    )
    .expect(crate::WRITE_TO_STRING);
    out
}

/// Trimmed section body following the first heading matching `re`.
fn section<'a>(md: &'a str, re: &str) -> &'a str {
    let heading_re = Regex::new(re).expect("valid regex");
    crate::markdown::section_after_heading(md, &heading_re)
}

#[cfg(test)]
mod tests;
