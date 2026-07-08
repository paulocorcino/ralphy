//! Trailer-based **plan resume**: the tiny convention that lets an abruptly-killed
//! run pick up a finalized `plan.md` and skip straight to execution instead of
//! re-planning from scratch.
//!
//! A planner writes [`plan_trailer`] as the VERY LAST line of `.ralphy/plan.md`
//! once every section is complete. [`plan_is_finalized_for`] treats that trailer,
//! present as the last non-empty line and carrying THIS issue's number, as the
//! "finalized" signal — the plan shell then keeps the file and does not re-run the
//! planner.

use std::fs;
use std::path::Path;

/// The finalized-plan marker for `issue_number`, written by the planner as the
/// last line of `.ralphy/plan.md`. Reuses the existing consolidate last-line
/// marker convention; the number carries plan identity.
pub fn plan_trailer(issue_number: u64) -> String {
    format!("<!-- ralphy-plan: issue={issue_number} -->")
}

/// Is the `plan.md` at `plan_path` a finalized plan for `issue_number`?
///
/// True only when the file's last non-empty (trimmed) line equals
/// [`plan_trailer`]. Any read error → `false` (re-plan). Detection uses the last
/// NON-empty line so a stray trailing blank an editor may append still resolves,
/// while a plan truncated mid-write (its real last line is prose) does not match.
///
/// Note: the resume window closes once the executor appends a section AFTER the
/// trailer (`## Notes & decisions`, `## Plan friction`, `## Handoff`) — the
/// trailer is then no longer the last line, so a later kill falls back to
/// re-planning. That is a safe degradation (a wasted re-plan, never an incorrect
/// resume); the common case — killed mid-execution before any append — resumes.
pub fn plan_is_finalized_for(plan_path: &Path, issue_number: u64) -> bool {
    let Ok(md) = fs::read_to_string(plan_path) else {
        return false;
    };
    md.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .is_some_and(|last| last.trim() == plan_trailer(issue_number))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ralphy-resume-{}-{}", std::process::id(), name))
    }

    #[test]
    fn trailer_as_last_line_is_finalized() {
        let p = tmp("last");
        fs::write(&p, "# plan\n<!-- ralphy-plan: issue=147 -->").unwrap();
        assert!(plan_is_finalized_for(&p, 147));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn trailing_blank_lines_tolerated() {
        let p = tmp("blank");
        fs::write(&p, "# plan\n<!-- ralphy-plan: issue=147 -->\n\n").unwrap();
        assert!(plan_is_finalized_for(&p, 147));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn other_issue_trailer_not_finalized() {
        let p = tmp("other");
        fs::write(&p, "# plan\n<!-- ralphy-plan: issue=999 -->").unwrap();
        assert!(!plan_is_finalized_for(&p, 147));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn truncated_plan_not_finalized() {
        let p = tmp("trunc");
        fs::write(&p, "# plan\n## Steps\n- [ ] half-written").unwrap();
        assert!(!plan_is_finalized_for(&p, 147));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn missing_file_not_finalized() {
        let p = tmp("missing-does-not-exist");
        assert!(!plan_is_finalized_for(&p, 147));
    }

    #[test]
    fn trailer_matches_literal_format() {
        assert_eq!(plan_trailer(147), "<!-- ralphy-plan: issue=147 -->");
    }
}
