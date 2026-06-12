//! Extracting the `## Handoff` / `## Plan friction` knowledge a session leaves
//! in its plan, and rendering predecessors' handoffs for the next session.
//! Pure functions over markdown strings — no I/O, no `gh` calls.
//!
//! The flow ("standing on the shoulders of giants"): the executor writes a
//! `## Handoff` section into `.ralphy/plan.md`; at close the runner publishes
//! it (plus `## Plan friction`) as an issue comment; before planning a
//! dependent issue, the runner collects those comments from the closed
//! blockers into `.ralphy/handoffs.md`, which the planner reads as leads.

use regex::Regex;

/// The marker heading a close-report comment carries. `find_handoff_comment`
/// keys on it, so publishing and collecting stay in lockstep.
const HANDOFF_HEADING: &str = "## Handoff";

/// Build the close-report comment from a plan: the `## Handoff` section plus
/// the `## Plan friction` section, each kept under its own heading. Returns
/// `None` when the plan carries neither (or only blank ones), so callers can
/// skip posting an empty comment.
pub fn close_report(plan_md: &str) -> Option<String> {
    let handoff = section(plan_md, r"(?im)^##\s+Handoff\s*$");
    let friction = section(plan_md, r"(?im)^##\s+Plan friction\s*$");

    let mut out = String::new();
    if !handoff.is_empty() {
        out.push_str(HANDOFF_HEADING);
        out.push_str("\n\n");
        out.push_str(handoff);
        out.push('\n');
    }
    if !friction.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("## Plan friction\n\n");
        out.push_str(friction);
        out.push('\n');
    }
    (!out.is_empty()).then_some(out)
}

/// Extract the planner's reasoning from an infeasible plan: the prose under
/// `## Feasible: ...` (the heading itself carries the verdict, so any tail
/// after "Feasible" is accepted). Returns `None` when the section is absent
/// or blank.
pub fn infeasible_reason(plan_md: &str) -> Option<String> {
    let reason = section(plan_md, r"(?im)^##\s+Feasible\b.*$");
    (!reason.is_empty()).then(|| reason.to_string())
}

/// Pick the handoff out of an issue's comments: the LAST comment containing a
/// `## Handoff` heading (a re-run of the issue supersedes earlier reports).
pub fn find_handoff_comment(comments: &[String]) -> Option<String> {
    let re = Regex::new(r"(?im)^##\s+Handoff\s*$").expect("valid regex");
    comments
        .iter()
        .rev()
        .find(|c| re.is_match(c))
        .map(|c| c.trim().to_string())
}

/// Render `.ralphy/handoffs.md` from the collected `(issue number, handoff
/// comment)` pairs. Returns `None` when there is nothing to write, so the
/// caller can remove a stale file instead.
pub fn render_handoffs_file(entries: &[(u64, String)]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut out = String::from(
        "# Handoffs from dependency issues\n\n\
         Knowledge left by the closed issues this one depends on. Entries are\n\
         leads, not truths — they were accurate when each issue closed.\n",
    );
    for (number, handoff) in entries {
        out.push_str(&format!("\n---\n\n## From #{number}\n\n{handoff}\n"));
    }
    Some(out)
}

/// Extract the durable-knowledge bullets from a plan's `## Handoff` section:
/// the `**Environment facts & traps**` and `**Commands that work**` entries,
/// verbatim with their sub-lines. `Delivered` and `Residue` are issue-specific
/// and stay out — they already travel to dependents via `handoffs.md`. Returns
/// `None` when the handoff is absent or carries neither, so the runner skips
/// writing an empty knowledge file.
pub fn knowledge_note(plan_md: &str) -> Option<String> {
    let handoff = section(plan_md, r"(?im)^##\s+Handoff\s*$");
    if handoff.is_empty() {
        return None;
    }
    let mut out = String::new();
    for key in ["Environment facts & traps", "Commands that work"] {
        let block = bullet_block(handoff, key);
        if !block.is_empty() {
            out.push_str(block);
            out.push('\n');
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The `- **<key>**...` bullet and everything under it, up to the next
/// top-level bold bullet (or end of input). Empty when the key is absent.
fn bullet_block<'a>(md: &'a str, key: &str) -> &'a str {
    let key_re =
        Regex::new(&format!(r"(?im)^\s*-\s*\*\*{}\*\*", regex::escape(key))).expect("valid regex");
    let Some(start) = key_re.find(md).map(|m| m.start()) else {
        return "";
    };
    let bullet_re = Regex::new(r"(?m)^\s*-\s*\*\*").expect("valid regex");
    let end = bullet_re
        .find_iter(md)
        .map(|m| m.start())
        .find(|&s| s > start)
        .unwrap_or(md.len());
    md[start..end].trim_end()
}

/// Trimmed section body following the first heading matching `re`.
fn section<'a>(md: &'a str, re: &str) -> &'a str {
    let heading_re = Regex::new(re).expect("valid regex");
    crate::markdown::section_after_heading(md, &heading_re).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_WITH_BOTH: &str = "\
# Plan for #2

## Steps
- [x] done

## Handoff

- **Delivered**: fixtures + Setup-Lab.ps1 (abc1234)
- **Environment facts & traps**: image 2ede8a0e does not process INVENTORY out of the box
- **Commands that work**: docker compose up -d; curl -I http://localhost:8080
- **Residue**: Setup-Lab.ps1 never ran clean-slate; run `docker compose down -v` then the script

## Plan friction

- the plan treated the lab as a given precondition; it was 70% of the work

## Notes & decisions
some note
";

    #[test]
    fn close_report_carries_handoff_and_friction() {
        let report = close_report(PLAN_WITH_BOTH).expect("report present");
        assert!(report.starts_with("## Handoff"));
        assert!(report.contains("Setup-Lab.ps1 never ran clean-slate"));
        assert!(report.contains("## Plan friction"));
        assert!(report.contains("70% of the work"));
        // Neighbouring sections must not leak in.
        assert!(!report.contains("Notes & decisions"));
        assert!(!report.contains("- [x] done"));
    }

    #[test]
    fn close_report_with_only_friction_keeps_friction_heading() {
        let md = "# Plan\n\n## Plan friction\n- none\n";
        let report = close_report(md).expect("report present");
        assert!(report.starts_with("## Plan friction"));
        assert!(report.contains("- none"));
    }

    #[test]
    fn close_report_absent_sections_is_none() {
        assert_eq!(close_report("# Plan\n\n## Steps\n- [x] x\n"), None);
    }

    #[test]
    fn close_report_blank_sections_is_none() {
        assert_eq!(
            close_report("# Plan\n\n## Handoff\n\n\n## Steps\n- [x] x\n"),
            None
        );
    }

    #[test]
    fn infeasible_reason_reads_feasible_section_with_verdict_tail() {
        let md = "# Plan for #3\n\n## Feasible: no\nThe issue bundles six PRD tasks; split into W1-T01..T06.\n\n## Steps\n";
        let reason = infeasible_reason(md).expect("reason present");
        assert!(reason.contains("bundles six PRD tasks"));
    }

    #[test]
    fn infeasible_reason_absent_is_none() {
        assert_eq!(infeasible_reason("# Plan\n\n## Steps\n"), None);
    }

    #[test]
    fn find_handoff_comment_picks_last_match() {
        let comments = vec![
            "## Acceptance ledger\n- **[verified]** x".to_string(),
            "## Handoff\n\n- **Delivered**: v1".to_string(),
            "just chatter".to_string(),
            "## Handoff\n\n- **Delivered**: v2 (re-run)".to_string(),
        ];
        let found = find_handoff_comment(&comments).expect("handoff present");
        assert!(found.contains("v2 (re-run)"));
    }

    #[test]
    fn find_handoff_comment_none_when_absent() {
        let comments = vec!["## Acceptance ledger\nstuff".to_string()];
        assert_eq!(find_handoff_comment(&comments), None);
    }

    #[test]
    fn render_handoffs_file_lists_each_dependency() {
        let entries = vec![
            (2u64, "## Handoff\n\n- **Delivered**: lab".to_string()),
            (5u64, "## Handoff\n\n- **Delivered**: schema".to_string()),
        ];
        let file = render_handoffs_file(&entries).expect("file content");
        assert!(file.contains("## From #2"));
        assert!(file.contains("## From #5"));
        assert!(file.contains("leads, not truths"));
    }

    #[test]
    fn render_handoffs_file_empty_is_none() {
        assert_eq!(render_handoffs_file(&[]), None);
    }

    #[test]
    fn knowledge_note_extracts_env_facts_and_commands_only() {
        let note = knowledge_note(PLAN_WITH_BOTH).expect("note present");
        assert!(note.contains("**Environment facts & traps**"));
        assert!(note.contains("does not process INVENTORY"));
        assert!(note.contains("**Commands that work**"));
        assert!(note.contains("docker compose up -d"));
        // Issue-specific entries stay out.
        assert!(!note.contains("**Delivered**"));
        assert!(!note.contains("**Residue**"));
        // Neighbouring sections must not leak in.
        assert!(!note.contains("Plan friction"));
    }

    #[test]
    fn knowledge_note_keeps_sub_lines_of_a_block() {
        let md = "\
## Handoff

- **Delivered**: stuff (abc1234)
- **Environment facts & traps**:
  - proxy strips the TAG header; pass it via query instead
  - schema rejects empty DEVICEID
- **Residue**: none
";
        let note = knowledge_note(md).expect("note present");
        assert!(note.contains("proxy strips the TAG header"));
        assert!(note.contains("schema rejects empty DEVICEID"));
        assert!(!note.contains("**Residue**"));
    }

    #[test]
    fn knowledge_note_none_without_durable_entries() {
        let md = "## Handoff\n\n- **Delivered**: docs only\n- **Residue**: none\n";
        assert_eq!(knowledge_note(md), None);
        assert_eq!(knowledge_note("# Plan\n\n## Steps\n- [x] x\n"), None);
    }
}
