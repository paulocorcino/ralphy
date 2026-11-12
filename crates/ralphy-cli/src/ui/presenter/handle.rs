use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use console::Style;
use indicatif::MultiProgress;

use super::{LiveState, PresenterMsg};
use crate::ui::{
    fit, queue_bar_label, render_info_line, render_totals_panel, PanelData, RenderOpts,
};

/// A teardown handle over the presenter's shared live region. `run_cmd` calls
/// [`finalize`](Self::finalize) after `run_queue` returns to flush the queue bar to
/// `N/N` and clear the bars, so the final summary `println!`s are not tangled with
/// a live region (ADR-0006 consequences).
pub(crate) struct PresenterHandle {
    pub(super) multi: MultiProgress,
    pub(super) state: Arc<Mutex<LiveState>>,
    pub(super) color: bool,
    /// Sender to the render thread so [`finalize`](Self::finalize) can flush the last
    /// scroll lines before clearing the region. `None` on the plain / banner path.
    pub(super) flush: Option<Sender<PresenterMsg>>,
    /// The run-border notice folded off the bus (#222), shared with
    /// [`crate::ui::EdgeNoticeLayer`]. Detached (and thus always empty) unless
    /// `init_tracing` installed the layer.
    pub(super) edge: Arc<Mutex<crate::ui::EdgeNoticeState>>,
}

impl PresenterHandle {
    /// A plain (colour off, no bars) handle for the `--verbose` / raw-stderr path.
    /// `finalize` is a no-op; `print_panel` produces uncoloured lines.
    pub(crate) fn plain() -> PresenterHandle {
        PresenterHandle {
            multi: MultiProgress::new(),
            state: Arc::new(Mutex::new(LiveState::default())),
            color: false,
            flush: None,
            edge: Arc::new(Mutex::new(crate::ui::EdgeNoticeState::default())),
        }
    }

    /// Attach the shared run-border notice state (`init_tracing` builds it with the
    /// layer). Without this the handle keeps its own detached, always-empty state.
    pub(crate) fn with_edge(mut self, edge: Arc<Mutex<crate::ui::EdgeNoticeState>>) -> Self {
        self.edge = edge;
        self
    }

    /// Print the run-border notice, if a border event folded one (#222). Same
    /// stdout stream and byte shape as the imperative print it replaced; a no-op
    /// on every run that did work. Call AFTER [`finalize`](Self::finalize), so the
    /// live region is cleared first (ADR-0006).
    pub(crate) fn print_edge_notice(&self) {
        let notice = match self.edge.lock() {
            Ok(mut g) => g.take(),
            Err(e) => e.into_inner().take(),
        };
        if let Some(text) = notice {
            outln!("{text}");
        }
    }

    /// Print the run's branding header (`🦊 Ralphy - vX`) at start-up, seeded by a
    /// stable per-run `seed` (the derived run title), so the face is identical to the
    /// Telegram card for the run and varies across runs. Routed through
    /// `MultiProgress` when styled so it sits cleanly above the live region; bold-cyan
    /// on a colour TTY, plain otherwise.
    pub(crate) fn print_header(&self, seed: &str) {
        let header = crate::runstate::ralphy_header(seed);
        if self.color {
            // the console is the only place to report a failed write to it
            match self
                .multi
                .println(Style::new().cyan().bold().apply_to(&header).to_string())
            {
                Ok(()) | Err(_) => {}
            }
        } else {
            outln!("{header}");
        }
    }

    /// Print the start-up info line (project · branch · repo URL) under the header.
    /// Routed through `MultiProgress` when styled so it sits above the live region.
    pub(crate) fn print_info_line(&self, project: &str, branch: Option<&str>, url: Option<&str>) {
        let opts = RenderOpts {
            color: self.color,
            emoji: self.color,
        };
        let line = render_info_line(project, branch, url, opts);
        if self.color {
            // the console is the only place to report a failed write to it
            match self.multi.println(line) {
                Ok(()) | Err(_) => {}
            }
        } else {
            outln!("{line}");
        }
    }

    /// Print the end-of-run totals panel to stdout, coloured when the handle is
    /// styled. Called after `finalize` has cleared the live region.
    pub(crate) fn print_panel(&self, data: &PanelData) {
        let opts = RenderOpts {
            color: self.color,
            emoji: self.color,
        };
        for line in render_totals_panel(data, opts) {
            outln!("{line}");
        }
    }

    /// Flush the queue bar to `N/N` (covering a trailing infeasible/dry-run issue
    /// with no following event) and clear the live region. No-op off a colour TTY.
    pub(crate) fn finalize(&self) {
        if !self.color {
            return;
        }
        // Drain the render thread so every scroll line is on screen before we clear
        // the region. Bounded: if the render thread is wedged in a stalled console
        // write, skip the clear rather than block teardown forever — the process is
        // exiting anyway.
        if let Some(tx) = self.flush.as_ref() {
            let (ack_tx, ack_rx) = mpsc::sync_channel(0);
            if tx.send(PresenterMsg::Flush(ack_tx)).is_err()
                || ack_rx.recv_timeout(Duration::from_secs(2)).is_err()
            {
                return;
            }
        }
        let Some(mut s) = self.lock_state_bounded(Duration::from_secs(2)) else {
            return;
        };
        // The end-of-run flush: a trailing dry-run issue has no following event to
        // supersede it, so `finished` is what takes the bar to N/N.
        s.run.finished = true;
        let label = queue_bar_label(
            &s.run,
            RenderOpts {
                color: false,
                emoji: true,
            },
            fit::terminal_width(),
        );
        if let Some(bar) = s.queue_bar.as_ref() {
            bar.set_message(label);
        }
        if let Some(bar) = s.active_bar.take() {
            bar.finish_and_clear();
        }
        if let Some(bar) = s.queue_bar.take() {
            bar.finish_and_clear();
        }
        // the console is the only place to report a failed write to it
        match self.multi.clear() {
            Ok(()) | Err(_) => {}
        }
    }

    /// Acquire the live state without blocking teardown forever: on a wedged render
    /// thread (holding the lock during a stalled console write) return `None` after
    /// `budget` instead of parking. Recovers from a poisoned lock.
    pub(super) fn lock_state_bounded(
        &self,
        budget: Duration,
    ) -> Option<std::sync::MutexGuard<'_, LiveState>> {
        let deadline = Instant::now() + budget;
        loop {
            match self.state.try_lock() {
                Ok(g) => return Some(g),
                Err(std::sync::TryLockError::Poisoned(e)) => return Some(e.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }
}
