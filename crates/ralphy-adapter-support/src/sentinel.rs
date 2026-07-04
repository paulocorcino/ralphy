//! Agent-protocol constants + done/blocked sentinel parsing.

/// The completion sentinel every Ralphy execution charter tells the agent to
/// emit — the single source of truth for the literal (the prose in
/// `assets/prompts/prompt.execute.md` must match). Detection stays in the
/// adapters; the core only ever receives this token as data to quote in its
/// repair briefs (ADR-0002 amendment, #79).
pub const DONE_SENTINEL: &str = "RALPHY_DONE_EXIT";

/// The one-line plan charter delivered per issue. It points the agent at the
/// full planning charter written to `.ralphy/plan-charter.md` at the top of
/// each plan call (mirroring `.ralphy/exec.md`) and restates the output
/// contract. Planning has no completion sentinel — the runner detects success
/// by `.ralphy/plan.md` appearing on disk — so unlike the exec charter this
/// names no sentinel.
pub const PLAN_CHARTER: &str = "Read .ralphy/plan-charter.md and follow it exactly to plan the issue described by .ralphy/issue.json. Write the plan to .ralphy/plan.md.";

/// The vendor-neutral execution charter, embedded once here (like [`PLAN_CHARTER`])
/// and referenced by every adapter, instead of each `include_str!`-ing its own
/// byte-identical copy. It already names the `RALPHY_DONE_EXIT` /
/// `RALPHY_BLOCKED_EXIT` sentinels and is not vendor-specific. The `../../../`
/// depth reaches the workspace root from this crate's `src/`, the same depth the
/// adapters used.
pub const PROMPT_EXECUTE: &str = include_str!("../../../assets/prompts/prompt.execute.md");

/// Returns `true` when `text` contains the [`DONE_SENTINEL`] token, as
/// defined by `assets/prompts/prompt.execute.md`.
pub fn done_sentinel(text: &str) -> bool {
    text.contains(DONE_SENTINEL)
}

/// Returns the trimmed reason from the first `RALPHY_BLOCKED_EXIT <reason>` line
/// in `text`, or `None` when no such line is present. A bare marker with no
/// trailing text yields `Some("")`.
pub fn blocked_reason(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.contains("RALPHY_BLOCKED_EXIT"))?;
    Some(
        line.split_once("RALPHY_BLOCKED_EXIT")
            .map(|(_, rest)| rest.trim().to_string())
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Anti-drift: the per-issue pointer charter must name the on-disk
    /// artifacts (charter, issue, plan output), must not invent a completion
    /// sentinel (plan success is `.ralphy/plan.md` appearing on disk), and
    /// must stay a pointer — never regrow into a full charter.
    #[test]
    fn plan_charter_points_at_disk_artifacts() {
        assert!(PLAN_CHARTER.contains(".ralphy/plan-charter.md"));
        assert!(PLAN_CHARTER.contains(".ralphy/issue.json"));
        assert!(PLAN_CHARTER.contains(".ralphy/plan.md"));
        assert!(
            !PLAN_CHARTER.contains(DONE_SENTINEL),
            "planning has no completion sentinel"
        );
        assert!(PLAN_CHARTER.len() < 512, "must stay a one-line pointer");
    }

    /// Anti-drift: the shared execution charter must name the completion
    /// sentinel (migrated from each adapter's local pin). `DONE_SENTINEL` is the
    /// single source of truth.
    #[test]
    fn prompt_execute_names_the_done_sentinel() {
        assert!(PROMPT_EXECUTE.contains(DONE_SENTINEL));
    }

    #[test]
    fn blocked_reason_extracts_trimmed_reason() {
        assert_eq!(
            blocked_reason("RALPHY_BLOCKED_EXIT missing key"),
            Some("missing key".into())
        );
    }

    #[test]
    fn done_sentinel_detects_bare_done() {
        assert!(done_sentinel("some output\nRALPHY_DONE_EXIT\n"));
    }

    #[test]
    fn neither_sentinel_yields_none_and_false() {
        let text = "no sentinel here";
        assert_eq!(blocked_reason(text), None);
        assert!(!done_sentinel(text));
    }

    #[test]
    fn blocked_reason_with_surrounding_whitespace_is_trimmed() {
        assert_eq!(
            blocked_reason("  RALPHY_BLOCKED_EXIT   need crate X  "),
            Some("need crate X".into())
        );
    }
}
