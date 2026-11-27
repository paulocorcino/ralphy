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

/// The finalized-plan marker for `issue_number`. The primary PRODUCER is the LLM
/// planner (instructed by the plan prompt to write this as the last line of
/// `.ralphy/plan.md`); as a fallback, [`crate::scaffold::run_plan_session`] stamps it
/// Rust-side when a planner exited cleanly but omitted it (some vendors reliably do).
/// [`plan_is_finalized_for`] is the DETECTOR. Exists so the prompt-const tests, the
/// stamper, and the detector all derive the exact literal from one place.
///
/// "Finalized" means "the planner finished writing" — NOT "the issue completed".
/// An executor may keep the trailer last after its own appends, so a run killed
/// after execution (in the gates, before the close) also resumes here. The runner
/// reads that plan's steps: when every step is checked it skips the executor and
/// runs the gates, so a finished plan is never executed again.
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
/// Note: when the executor appends a section AFTER the trailer (`## Notes &
/// decisions`, `## Plan friction`, `## Handoff`), the trailer is no longer the
/// last line and a later kill falls back to re-planning — a wasted re-plan,
/// never an incorrect resume.
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
    use crate::scratch::remove_scratch_file;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ralphy-resume-{}-{}", std::process::id(), name))
    }

    #[test]
    fn plan_finalized_table() {
        let rows: [(&str, Option<&str>, bool); 5] = [
            (
                "trailer as last line",
                Some(
                    "# plan
<!-- ralphy-plan: issue=147 -->",
                ),
                true,
            ),
            (
                "trailing blank lines",
                Some(
                    "# plan
<!-- ralphy-plan: issue=147 -->

",
                ),
                true,
            ),
            (
                "other issue",
                Some(
                    "# plan
<!-- ralphy-plan: issue=999 -->",
                ),
                false,
            ),
            (
                "truncated plan",
                Some(
                    "# plan
## Steps
- [ ] half-written",
                ),
                false,
            ),
            ("missing file", None, false),
        ];
        for (name, content, want) in rows {
            let p = tmp(&name.replace(' ', "-"));
            if let Some(text) = content {
                fs::write(&p, text).unwrap();
            }
            assert_eq!(plan_is_finalized_for(&p, 147), want, "{name}");
            remove_scratch_file(&p);
        }
        assert_eq!(plan_trailer(147), "<!-- ralphy-plan: issue=147 -->");
    }
}
