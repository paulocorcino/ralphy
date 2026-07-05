//! Per-issue deadline arithmetic (D1), shared by every adapter. The clock math is
//! identical across vendors and carries zero vendor content: `now + budget`, where
//! `budget` is either the per-issue cap or a caller-supplied `unbounded` horizon,
//! then clamped to the run-level deadline when one is set.

use std::time::{Duration, Instant};

/// The deadline for the current issue: `now + max_minutes_per_issue`, clamped to
/// `run_deadline` when one is set. A `max_minutes_per_issue` of `0` disables the
/// per-issue cap — the budget falls back to `unbounded` (the far-future horizon the
/// caller sources from core, `ralphy_core::UNBOUNDED_ISSUE_HORIZON`, so this crate
/// stays core-free), and the issue is then bounded only by `run_deadline`.
pub fn issue_deadline(
    now: Instant,
    max_minutes_per_issue: u64,
    run_deadline: Option<Instant>,
    unbounded: Duration,
) -> Instant {
    let budget = if max_minutes_per_issue == 0 {
        unbounded
    } else {
        Duration::from_secs(max_minutes_per_issue * 60)
    };
    let per_issue = now + budget;
    match run_deadline {
        Some(rd) => per_issue.min(rd),
        None => per_issue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The horizon core substitutes when the per-issue cap is disabled; kept in
    // sync with `ralphy_core::UNBOUNDED_ISSUE_HORIZON` by construction here (this
    // crate stays core-free, so the constant is passed in by the adapters).
    const UNBOUNDED: Duration = Duration::from_secs(365 * 24 * 60 * 60);

    #[test]
    fn uncapped_beats_a_finite_budget() {
        let now = Instant::now();
        let uncapped = issue_deadline(now, 0, None, UNBOUNDED);
        let capped = issue_deadline(now, 1000, None, UNBOUNDED);
        assert!(
            uncapped > capped,
            "a 0-minute (unbounded) deadline sits past any finite budget"
        );
    }

    #[test]
    fn a_near_run_deadline_clamps_a_large_budget() {
        let now = Instant::now();
        let rd = now + Duration::from_secs(1);
        assert!(
            issue_deadline(now, 1000, Some(rd), UNBOUNDED) <= rd,
            "a large per-issue budget is clamped to the nearer run deadline"
        );
    }

    #[test]
    fn zero_minutes_is_still_bounded_by_the_run_deadline() {
        let now = Instant::now();
        let rd = now + Duration::from_secs(1);
        assert!(
            issue_deadline(now, 0, Some(rd), UNBOUNDED) <= rd,
            "an uncapped issue is still bounded by the run deadline when set"
        );
    }
}
