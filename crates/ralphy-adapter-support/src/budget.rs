//! Per-issue deadline arithmetic (D1), shared by every adapter. The clock math is
//! identical across vendors and carries zero vendor content: `now + budget`, where
//! `budget` is either the per-issue cap or a caller-supplied `unbounded` horizon,
//! then clamped to the run-level deadline when one is set.

use std::time::{Duration, Instant};

/// The per-issue wall-clock budget every adapter carries: the per-issue cap plus
/// the optional run-level deadline it is clamped to. The clock math is
/// vendor-neutral, so the builder trio and the [`deadline`](Self::deadline) /
/// [`timeout`](Self::timeout) arithmetic live here once and each adapter holds one
/// of these instead of the two loose fields. `unbounded` (the far-future horizon a
/// `0` cap falls back to) is passed in by the caller so this crate stays core-free.
#[derive(Debug, Clone, Copy)]
pub struct IssueBudget {
    /// The per-issue cap in minutes; `0` disables it (falls back to `unbounded`).
    pub max_minutes_per_issue: u64,
    /// The run-level deadline the per-issue budget is clamped to, when set.
    pub run_deadline: Option<Instant>,
}

impl IssueBudget {
    /// A budget with the given default per-issue cap and no run deadline.
    pub fn new(default_max_minutes: u64) -> Self {
        Self {
            max_minutes_per_issue: default_max_minutes,
            run_deadline: None,
        }
    }

    /// Set the per-issue wall-clock budget in minutes.
    pub fn with_max_minutes_per_issue(mut self, minutes: u64) -> Self {
        self.max_minutes_per_issue = minutes;
        self
    }

    /// Set the run's global wall-clock deadline; each issue's budget is clamped to it.
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.run_deadline = run_deadline;
        self
    }

    /// The deadline for the current issue (see [`issue_deadline`]), clocked from now.
    pub fn deadline(&self, unbounded: Duration) -> Instant {
        issue_deadline(
            Instant::now(),
            self.max_minutes_per_issue,
            self.run_deadline,
            unbounded,
        )
    }

    /// The remaining wall-clock budget: the deadline less now, saturating at zero.
    pub fn timeout(&self, unbounded: Duration) -> Duration {
        self.deadline(unbounded)
            .saturating_duration_since(Instant::now())
    }
}

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

    // ── IssueBudget ─────────────────────────────────────────────────────────

    #[test]
    fn issue_budget_new_seeds_the_default_cap() {
        assert_eq!(IssueBudget::new(120).max_minutes_per_issue, 120);
        assert!(IssueBudget::new(120).run_deadline.is_none());
    }

    #[test]
    fn issue_budget_uncapped_deadline_beats_a_finite_one() {
        let uncapped = IssueBudget::new(0).with_max_minutes_per_issue(0);
        let capped = IssueBudget::new(0).with_max_minutes_per_issue(1000);
        assert!(uncapped.deadline(UNBOUNDED) > capped.deadline(UNBOUNDED));
    }

    #[test]
    fn issue_budget_deadline_clamps_to_the_run_deadline() {
        let rd = Instant::now() + Duration::from_secs(1);
        let clamped = IssueBudget::new(0)
            .with_max_minutes_per_issue(1000)
            .with_run_deadline(Some(rd));
        assert!(clamped.deadline(UNBOUNDED) <= rd);
    }
}
