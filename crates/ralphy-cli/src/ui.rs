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

/// The pure state behind the queue progress bar: a fixed `total`, the ordered
/// `pending` issue numbers, and a `completed` count. Every terminal outcome
/// advances it by one; [`finish`](Self::finish) flushes it to `N/N`. Kept pure
/// (no `indicatif`) so the advancement logic is unit-tested directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueState {
    total: u64,
    pending: Vec<u64>,
    completed: u64,
    /// The first issue carrying `stop-before`: the run halts before it, so the
    /// pending bar marks the cut. `None` when no queue issue is tagged.
    stop_before: Option<u64>,
}

impl QueueState {
    /// Build from the `queue built` `count`, parsed `order`, and `stop_before`
    /// boundary (the first `stop-before` issue, or `None`).
    pub fn built(count: u64, order: Vec<u64>, stop_before: Option<u64>) -> Self {
        QueueState {
            total: count,
            pending: order,
            completed: 0,
            stop_before,
        }
    }

    /// A terminal outcome for `number` (done / non-green / blocked / stop-before):
    /// drop it from `pending` and bump `completed`. Idempotent — a `number` not in
    /// `pending` (already advanced, or superseded) is a no-op, so a double event
    /// never over-counts.
    pub fn advance(&mut self, number: u64) {
        if let Some(pos) = self.pending.iter().position(|&n| n == number) {
            self.pending.remove(pos);
            self.completed += 1;
        }
    }

    /// A new active issue started, so a still-pending prior issue that emitted no
    /// terminal event (infeasible / dry-run plan) is complete. Same as
    /// [`advance`](Self::advance).
    pub fn supersede(&mut self, number: u64) {
        self.advance(number);
    }

    /// Flush to `N/N` at the end of the run — covers a trailing infeasible/dry-run
    /// issue whose completion has no following `issue started` to supersede it.
    pub fn finish(&mut self) {
        self.completed = self.total;
        self.pending.clear();
    }

    /// Render `▰▰▰▱▱▱ 3/6 (pending #4 #5 #6)` with emoji on. ANSI-free by
    /// construction. Thin wrapper over [`bar_label_opts`](Self::bar_label_opts).
    pub fn bar_label(&self) -> String {
        self.bar_label_opts(RenderOpts {
            color: false,
            emoji: true,
        })
    }

    /// Render the queue bar, marking where a `stop-before` halts the run: the
    /// tagged issue is prefixed in the pending list (`… #10 ⛔ stop-before #15 …`),
    /// so the operator sees up front that nothing from that issue onward will run
    /// this session. `opts.emoji` picks the glyph; the bar itself is ANSI-free.
    pub fn bar_label_opts(&self, opts: RenderOpts) -> String {
        let done = self.completed.min(self.total) as usize;
        let left = (self.total.saturating_sub(self.completed)) as usize;
        let filled = "▰".repeat(done);
        let empty = "▱".repeat(left);
        let pending = if self.pending.is_empty() {
            String::new()
        } else {
            let nums: Vec<String> = self
                .pending
                .iter()
                .map(|&n| {
                    if Some(n) == self.stop_before {
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
        format!("{filled}{empty} {}/{}{pending}", self.completed, self.total)
    }
}

#[cfg(test)]
mod tests;
