//! The read-only views of a [`RunState`]: counts, the active issue, the run phase.

use super::{Counts, IssueEntry, IssueStatus, RunState};

impl RunState {
    /// Tally the issues by status for the counter line.
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for e in &self.issues {
            match e.status {
                IssueStatus::Planned => c.planned += 1,
                IssueStatus::Done => c.done += 1,
                IssueStatus::Skipped => c.skipped += 1,
                IssueStatus::Blocked => c.blocked += 1,
                IssueStatus::Infeasible => c.infeasible += 1,
                IssueStatus::NeedsSplit => c.needs_split += 1,
                IssueStatus::NonGreen => c.non_green += 1,
                IssueStatus::Planning => c.planning += 1,
                IssueStatus::Executing => c.executing += 1,
                IssueStatus::Hitl => c.hitl += 1,
            }
        }
        c
    }

    /// The active issue entry, if any.
    pub fn active_issue(&self) -> Option<&IssueEntry> {
        let n = self.active?;
        self.issues.iter().find(|e| e.number == n)
    }

    /// The most-recently-seen issue in a terminal status (for the collapsed card).
    pub fn most_recent_finished(&self) -> Option<&IssueEntry> {
        self.issues.iter().rev().find(|e| e.status.is_terminal())
    }

    /// The run's current phase — `starting|planning|executing|sleeping|
    /// consolidating`: a usage-limit sleep wins, then an in-progress
    /// consolidation, then the active issue's phase, else the initial
    /// `starting`. A sleep reports `sleeping` even with an executing issue so a
    /// long usage-limit pause is never mistaken for progress or for death.
    ///
    /// One vocabulary, read by the `run.heartbeat` envelope (ADR-0019) and by
    /// the run snapshot's phase block (ADR-0047 §5).
    pub fn run_phase(&self) -> &'static str {
        if self.sleep.is_some() {
            return "sleeping";
        }
        if self.consolidating.is_some() {
            return "consolidating";
        }
        match self.active_issue().map(|e| &e.status) {
            Some(IssueStatus::Executing) => "executing",
            Some(IssueStatus::Planning) => "planning",
            _ => "starting",
        }
    }
}
