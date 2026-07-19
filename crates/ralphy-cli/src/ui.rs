//! The console UI: renders the run's lifecycle as styled, local-timestamped
//! lines. The entire UI lives here (ADR-0006); the core stays a queue engine
//! that happens to log.
//!
//! The seam is thin: `on_event` (in [`presenter`]) calls
//! `runstate::event_to_runevent` to decode the raw tracing event into a
//! `RunEvent`, then passes it to `Presenter::apply`. The presenter owns the
//! side effects — timestamps, per-issue duration, and writing through
//! `indicatif`'s `MultiProgress` so warn/error lines never corrupt live output.

use console::Style;

mod notice;
mod presenter;
mod render;
pub(crate) use notice::{EdgeNoticeLayer, EdgeNoticeState};
pub use presenter::{Presenter, PresenterHandle};
pub use render::{
    normalize_remote_url, render_info_line, render_totals_panel, PanelBranchMode, PanelData,
    PanelStop, RenderOpts,
};
// Re-exported because it appears in `PanelData`'s public fields (constructed in `main`).
pub use crate::runstate::UsageLite;
use crate::runstate::{IssueStatus, RunState};
#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
use chrono::{DateTime, Local};
#[cfg(test)]
use render::{fmt_clock, fmt_duration, fmt_tokens, fmt_usd_compact, Meter};
#[cfg(test)]
use render::{render_active_line, render_line, sleep_label, LineExtra};

/// Which phase the active issue is in, for the live active-line icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Planning,
    Executing,
}

/// The terminal outcome of a finished issue, derived from the lifecycle event.
/// Mirrors `ralphy_core::Outcome` but kept UI-local so the presenter never
/// depends on the core's type (ADR-0006: the UI is a pure CLI concern).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishOutcome {
    Done,
    Blocked,
    Timeout,
    Stuck,
    Limit,
}

impl FinishOutcome {
    /// Short lower-case label for the rendered line.
    pub(crate) fn label(self) -> &'static str {
        match self {
            FinishOutcome::Done => "done",
            FinishOutcome::Blocked => "blocked",
            FinishOutcome::Timeout => "timeout",
            FinishOutcome::Stuck => "stuck",
            FinishOutcome::Limit => "limit",
        }
    }

    /// `(emoji, ascii fallback, semantic colour)` — the ADR-0006 D4 icon table.
    pub(crate) fn glyph(self) -> (&'static str, &'static str, Style) {
        match self {
            FinishOutcome::Done => ("✅", "[ok]", Style::new().green()),
            FinishOutcome::Blocked => ("⛔", "[blocked]", Style::new().red()),
            FinishOutcome::Timeout => ("⏱️", "[timeout]", Style::new().yellow()),
            FinishOutcome::Stuck => ("🪨", "[stuck]", Style::new().red()),
            FinishOutcome::Limit => ("🌙", "[limit]", Style::new().yellow()),
        }
    }
}

/// The phase icon for the live active line, derived from the folded status. `None`
/// for every terminal status — the issue is finished, so there is no active line.
pub(crate) fn active_phase(status: &IssueStatus) -> Option<Phase> {
    match status {
        IssueStatus::Planning => Some(Phase::Planning),
        IssueStatus::Executing => Some(Phase::Executing),
        IssueStatus::Planned
        | IssueStatus::Done
        | IssueStatus::Skipped
        | IssueStatus::Blocked
        | IssueStatus::Infeasible
        | IssueStatus::NeedsSplit
        | IssueStatus::NonGreen
        | IssueStatus::Hitl => None,
    }
}

/// Render the queue bar `▰▰▰▱▱▱ 3/6 (pending #4 #5 #6)` from the folded run state
/// (ADR-0007 D6 amendment #223: the console has no reducer of its own). An issue is
/// pending until its entry reaches a terminal status, so a plan-only pass advances
/// the bar the moment the fold supersedes it; a `finished` run flushes to `N/N`.
///
/// The `stop-before` cut is marked in the pending list (`… #10 ⛔ stop-before #15 …`)
/// so the operator sees up front that nothing from that issue onward will run this
/// session. `opts.emoji` picks the glyph; the bar itself is ANSI-free by construction.
pub(crate) fn queue_bar_label(state: &RunState, opts: RenderOpts) -> String {
    let terminal = |n: u64| {
        state
            .issues
            .iter()
            .any(|e| e.number == n && e.status.is_terminal())
    };
    let pending: Vec<u64> = if state.finished {
        Vec::new()
    } else {
        state
            .order
            .iter()
            .copied()
            .filter(|&n| !terminal(n))
            .collect()
    };
    let total = state.total;
    let completed = if state.finished {
        total
    } else {
        state.order.len().saturating_sub(pending.len())
    };
    let filled = "▰".repeat(completed.min(total));
    let empty = "▱".repeat(total.saturating_sub(completed));
    let pending_part = if pending.is_empty() {
        String::new()
    } else {
        let nums: Vec<String> = pending
            .iter()
            .map(|&n| {
                if Some(n) == state.stop_before {
                    // The cut: this issue (and everything after it) won't run.
                    if opts.emoji {
                        format!("⛔ stop-before #{n}")
                    } else {
                        format!("|stop-before #{n}|")
                    }
                } else {
                    format!("#{n}")
                }
            })
            .collect();
        format!(" (pending {})", nums.join(" "))
    };
    format!("{filled}{empty} {completed}/{total}{pending_part}")
}

#[cfg(test)]
mod tests;
