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

mod fit;
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
use crate::pricing::PriceTable;
#[cfg(test)]
use chrono::{DateTime, Local};
#[cfg(test)]
use render::{fmt_clock, fmt_duration, fmt_tokens, fmt_usd_compact, Meter};
#[cfg(test)]
use render::{meter_for, render_active_line, render_line, sleep_label, LineExtra};

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
pub(crate) fn queue_bar_label(state: &RunState, opts: RenderOpts, width: usize) -> String {
    let pending = pending_numbers(state);
    let total = state.total;
    let completed = if state.finished {
        total
    } else {
        state.order.len().saturating_sub(pending.len())
    };
    let filled = "▰".repeat(completed.min(total));
    let empty = "▱".repeat(total.saturating_sub(completed));
    let pending_part = pending_tail(&pending, state.stop_before, opts.emoji);
    let head = format!("{filled}{empty} {completed}/{total}");
    if fit::display_width(&head) > width {
        return shrink_bar(&filled, &empty, &format!(" {completed}/{total}"), width);
    }
    let head_width = fit::display_width(&head);
    head + &fit::truncate_to_width(&pending_part, width - head_width)
}

/// The queue's issues, in queue order, that have not reached a terminal
/// status; none once the run is finished.
fn pending_numbers(state: &RunState) -> Vec<u64> {
    if state.finished {
        return Vec::new();
    }
    let terminal = |n: u64| {
        state
            .issues
            .iter()
            .any(|e| e.number == n && e.status.is_terminal())
    };
    state
        .order
        .iter()
        .copied()
        .filter(|&n| !terminal(n))
        .collect()
}

/// The ` (pending #4 #5 #6)` tail, with the `stop-before` cut marked; empty
/// when nothing is pending.
fn pending_tail(pending: &[u64], stop_before: Option<u64>, emoji: bool) -> String {
    if pending.is_empty() {
        return String::new();
    }
    let nums: Vec<String> = pending
        .iter()
        .map(|&n| {
            if Some(n) == stop_before {
                // The cut: this issue (and everything after it) won't run.
                if emoji {
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
}

/// The bar glyphs overflow even before the pending tail: shrink them one at a
/// time, but never touch the `N/M` counter (the one segment that must survive
/// any width, per the issue's design constraint).
fn shrink_bar(filled: &str, empty: &str, counter: &str, width: usize) -> String {
    let bar_glyphs: Vec<char> = filled.chars().chain(empty.chars()).collect();
    for cut in (0..=bar_glyphs.len()).rev() {
        let bar: String = bar_glyphs[..cut].iter().collect();
        let candidate = format!("{bar}{counter}");
        if fit::display_width(&candidate) <= width {
            return candidate;
        }
    }
    counter.to_string() // Even the bare counter overflows: return it, never empty.
}

#[cfg(test)]
mod queue_bar_tests;
#[cfg(test)]
mod tests;
