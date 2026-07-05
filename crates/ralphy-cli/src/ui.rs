//! The console presenter: a `tracing_subscriber::Layer` that consumes the events
//! the core and adapters already emit and renders the run's lifecycle as styled,
//! local-timestamped lines. The entire UI lives here (ADR-0006); the core stays a
//! queue engine that happens to log.
//!
//! The seam is thin: [`on_event`] calls `runstate::event_to_runevent` to decode the
//! raw tracing event into a [`RunEvent`], then passes it to [`Presenter::apply`].
//! The presenter owns the side effects — timestamps, per-issue duration, and writing
//! through `indicatif`'s `MultiProgress` so warn/error lines never corrupt live output.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Local;
use console::Style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::pricing::PriceTable;
use crate::runstate::{event_to_runevent, EventFields, RunEvent};
// Re-exported because it appears in `PanelData`'s public fields (constructed in `main`).
pub use crate::runstate::UsageLite;

mod render;
#[cfg(test)]
use chrono::DateTime;
#[cfg(test)]
use render::{fmt_clock, fmt_duration, fmt_tokens, fmt_usd_compact, Meter};
pub use render::{
    normalize_remote_url, render_info_line, render_totals_panel, PanelBranchMode, PanelData,
    PanelStop, RenderOpts,
};
// Temporary: `Presenter` still lives here until it moves to `ui/presenter.rs`.
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

/// The active issue's live state, tracked so the finishing line can show its
/// wall-clock duration and the active spinner line its phase/model/budget.
struct ActiveIssue {
    number: u64,
    title: String,
    start: Instant,
    phase: Phase,
    model: Option<String>,
    effort: Option<String>,
    budget_min: Option<u64>,
    /// The planning phase's usage, stashed at `PlanWritten` so the `done` line can
    /// show the issue total (plan + execute) and price each phase's model (D8).
    plan_usage: Option<UsageLite>,
}

/// The mutable live-region state behind the presenter's single `Mutex`: the active
/// issue, the pure queue state, and the two `indicatif` bars (only present on a
/// colour TTY). Shared with the [`PresenterHandle`] via `Arc` so `main` can flush
/// and clear the region after the queue returns.
#[derive(Default)]
struct LiveState {
    active: Option<ActiveIssue>,
    queue: Option<QueueState>,
    sleep: Option<String>,
    queue_bar: Option<ProgressBar>,
    active_bar: Option<ProgressBar>,
}

/// The console presenter: a `tracing` Layer that renders the run's lifecycle. It
/// holds the active issue (so a finishing line can show the issue's wall-clock
/// duration) and a live `MultiProgress` region (queue bar + active spinner) behind
/// a shared `Mutex`, so on-screen lines never corrupt one another.
pub struct Presenter {
    multi: MultiProgress,
    state: Arc<Mutex<LiveState>>,
    opts: RenderOpts,
    /// The read-time price table, used to project per-line USD for the `plan
    /// written` and `done` meters (ADR-0008 D8). Loaded once at construction.
    price: PriceTable,
}

impl Presenter {
    /// Build a presenter, auto-detecting the terminal: styled (colour + emoji)
    /// on an attended TTY without `NO_COLOR`, else a plain clean-line renderer.
    pub fn new() -> Self {
        let is_tty = console::Term::stderr().is_term();
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let styled = is_tty && !no_color;
        let presenter = Presenter {
            multi: MultiProgress::new(),
            state: Arc::new(Mutex::new(LiveState::default())),
            opts: RenderOpts {
                color: styled,
                emoji: styled,
            },
            price: PriceTable::load(),
        };
        // On a styled TTY, repaint the active line every second so the elapsed clock
        // advances during quiet multi-minute execution stretches that emit no events.
        // Detached: the process exit at end-of-run tears it down (it only ever reads
        // shared state and updates an existing bar's message).
        if styled {
            let state = Arc::clone(&presenter.state);
            let opts = presenter.opts;
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(1));
                let s = state.lock().unwrap_or_else(|e| e.into_inner());
                repaint_active_bar(&s, opts);
            });
        }
        presenter
    }

    /// A teardown handle over the shared live region. `init_tracing` hands this to
    /// `run_cmd` so the queue bar is flushed to `N/N` and the live region cleared
    /// before the summary prints (ADR-0006: the presenter owns teardown).
    pub fn handle(&self) -> PresenterHandle {
        PresenterHandle {
            multi: self.multi.clone(),
            state: Arc::clone(&self.state),
            color: self.opts.color,
        }
    }

    /// Apply one decoded run event: drive the live region + active-issue tracking,
    /// then emit the permanent line (if any).
    fn apply(&self, event: RunEvent) {
        let ts = Local::now();
        // Recover from poison rather than panic: this runs inside `on_event`, so a
        // panic here would corrupt the run on a tracing call.
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());

        let extra = self.drive(&mut s, &event);

        // Styled lines on a colour TTY (routed through `MultiProgress` so they
        // never tear the live region); one clean, ANSI-free line per event
        // otherwise (the non-TTY / `NO_COLOR` path, ADR-0006 D3). `render_line`
        // styles per `opts.color`, so both paths share it — the plain path keeps
        // the same model/effort/meter tail.
        let line = render_line(&event, &ts, &extra, self.opts);
        if let Some(line) = line {
            if self.opts.color {
                let _ = self.multi.println(line);
            } else {
                eprintln!("{line}");
            }
        }
    }

    /// Update the live region for one event and return a finishing duration when
    /// the event closes the active issue. The live-region (`indicatif`) calls are
    /// guarded behind `self.opts.color`, so `--verbose`/non-TTY draw nothing.
    fn drive(&self, s: &mut LiveState, event: &RunEvent) -> LineExtra {
        match event {
            RunEvent::QueueBuilt {
                count,
                order,
                stop_before,
                ..
            } => {
                s.queue = Some(QueueState::built(*count, order.clone(), *stop_before));
                if self.opts.color {
                    let bar = self.multi.add(ProgressBar::new_spinner());
                    bar.set_style(ProgressStyle::with_template("{msg}").expect("static template"));
                    if let Some(q) = s.queue.as_ref() {
                        bar.set_message(q.bar_label_opts(self.opts));
                    }
                    s.queue_bar = Some(bar);
                }
                LineExtra::default()
            }
            RunEvent::IssueStarted { number, title } => {
                // A new active issue supersedes a still-pending prior one that
                // emitted no terminal event (infeasible / dry-run plan).
                if let Some(prev) = s.active.take() {
                    if let Some(q) = s.queue.as_mut() {
                        q.supersede(prev.number);
                    }
                    self.refresh_queue_bar(s);
                }
                s.active = Some(ActiveIssue {
                    number: *number,
                    title: title.clone(),
                    start: Instant::now(),
                    phase: Phase::Planning,
                    model: None,
                    effort: None,
                    budget_min: None,
                    plan_usage: None,
                });
                self.refresh_active_bar(s);
                LineExtra::default()
            }
            RunEvent::Planning { model, effort } => {
                // Live-region only: label the planning spinner with the planner's
                // display model/effort. The adapter carries no issue number.
                if let Some(a) = s.active.as_mut() {
                    if model.is_some() {
                        a.model = model.clone();
                    }
                    if effort.is_some() {
                        a.effort = effort.clone();
                    }
                }
                self.refresh_active_bar(s);
                LineExtra::default()
            }
            RunEvent::PlanWritten { usage, .. } => {
                // Stash the planning usage for the eventual `done` line, and surface
                // the scroll line with the planning meter + elapsed-so-far.
                match s.active.as_mut() {
                    Some(a) => {
                        a.plan_usage = Some(usage.clone());
                        LineExtra {
                            duration: Some(a.start.elapsed()),
                            model: a.model.clone(),
                            effort: a.effort.clone(),
                            meter: Some(meter_for(&self.price, None, usage)),
                        }
                    }
                    None => LineExtra {
                        meter: Some(meter_for(&self.price, None, usage)),
                        ..Default::default()
                    },
                }
            }
            RunEvent::Executing {
                model,
                budget_min,
                effort,
                ..
            } => {
                // The event carries no number; it applies to the active issue.
                if let Some(a) = s.active.as_mut() {
                    a.phase = Phase::Executing;
                    a.model = Some(model.clone());
                    if effort.is_some() {
                        a.effort = effort.clone();
                    }
                    a.budget_min = Some(*budget_min);
                }
                self.refresh_active_bar(s);
                LineExtra::default()
            }
            RunEvent::IssueClosed { number, usage, .. } => {
                // The `done` line shows the issue total (plan + execute) and prices
                // each phase's model: combine the stashed planning usage with this
                // execution usage before clearing the active issue.
                let extra = match s.active.as_ref().filter(|a| a.number == *number) {
                    Some(a) => LineExtra {
                        duration: Some(a.start.elapsed()),
                        model: a.model.clone(),
                        effort: a.effort.clone(),
                        meter: Some(meter_for(&self.price, a.plan_usage.as_ref(), usage)),
                    },
                    None => LineExtra::default(),
                };
                s.active = None;
                if let Some(q) = s.queue.as_mut() {
                    q.advance(*number);
                }
                self.refresh_queue_bar(s);
                if let Some(bar) = s.active_bar.take() {
                    bar.finish_and_clear();
                }
                extra
            }
            RunEvent::NonGreen { number, .. }
            | RunEvent::Skipped { number, .. }
            | RunEvent::HumanBlocked { number, .. } => {
                let duration = s
                    .active
                    .as_ref()
                    .filter(|a| a.number == *number)
                    .map(|a| a.start.elapsed());
                s.active = None;
                if let Some(q) = s.queue.as_mut() {
                    q.advance(*number);
                }
                self.refresh_queue_bar(s);
                if let Some(bar) = s.active_bar.take() {
                    bar.finish_and_clear();
                }
                LineExtra {
                    duration,
                    ..Default::default()
                }
            }
            RunEvent::SleepStarted { reset, .. } => {
                s.sleep = Some(reset.clone());
                if let Some(bar) = s.active_bar.take() {
                    bar.finish_and_clear();
                }
                self.refresh_queue_bar(s);
                LineExtra::default()
            }
            RunEvent::SleepEnded => {
                s.sleep = None;
                self.refresh_queue_bar(s);
                self.refresh_active_bar(s);
                LineExtra::default()
            }
            _ => LineExtra::default(),
        }
    }

    /// Repaint the queue bar's label from the current [`QueueState`]. No-op off a
    /// colour TTY.
    fn refresh_queue_bar(&self, s: &LiveState) {
        if !self.opts.color {
            return;
        }
        if let Some(bar) = s.queue_bar.as_ref() {
            let msg = match (&s.sleep, &s.queue) {
                (Some(reset), _) => sleep_label(reset, self.opts),
                (None, Some(q)) => q.bar_label_opts(self.opts),
                (None, None) => return,
            };
            bar.set_message(msg);
        }
    }

    /// Repaint (creating on first use) the self-ticking active-issue spinner from
    /// the current [`ActiveIssue`]. No-op off a colour TTY. The message itself is
    /// rendered by [`repaint_active_bar`], shared with the per-second clock ticker.
    fn refresh_active_bar(&self, s: &mut LiveState) {
        if !self.opts.color || s.active.is_none() {
            return;
        }
        if s.active_bar.is_none() {
            let b = self.multi.add(ProgressBar::new_spinner());
            b.set_style(
                ProgressStyle::with_template("{spinner:.cyan} {msg}").expect("static template"),
            );
            // Self-tick so the spinner keeps moving through quiet multi-minute
            // execution stretches that emit no events (ADR-0006 D4).
            b.enable_steady_tick(Duration::from_millis(120));
            s.active_bar = Some(b);
        }
        repaint_active_bar(s, self.opts);
    }
}

/// Repaint the active-issue spinner's message from the current state, recomputing
/// the elapsed clock. `indicatif`'s steady tick only re-draws the *same* message, so
/// the clock would freeze at its event-time value; calling this on a one-second
/// timer keeps the elapsed time advancing between events (ADR-0006 D4).
fn repaint_active_bar(s: &LiveState, opts: RenderOpts) {
    if let (Some(a), Some(bar)) = (s.active.as_ref(), s.active_bar.as_ref()) {
        bar.set_message(render_active_line(
            a.phase,
            a.number,
            &a.title,
            a.model.as_deref(),
            a.effort.as_deref(),
            a.start.elapsed(),
            a.budget_min,
            opts,
        ));
    }
}

impl Default for Presenter {
    fn default() -> Self {
        Self::new()
    }
}

/// A teardown handle over the presenter's shared live region. `run_cmd` calls
/// [`finalize`](Self::finalize) after `run_queue` returns to flush the queue bar to
/// `N/N` and clear the bars, so the final summary `println!`s are not tangled with
/// a live region (ADR-0006 consequences).
pub struct PresenterHandle {
    multi: MultiProgress,
    state: Arc<Mutex<LiveState>>,
    color: bool,
}

impl PresenterHandle {
    /// A plain (colour off, no bars) handle for the `--verbose` / raw-stderr path.
    /// `finalize` is a no-op; `print_panel`/`print_notice` produce uncoloured lines.
    pub fn plain() -> PresenterHandle {
        PresenterHandle {
            multi: MultiProgress::new(),
            state: Arc::new(Mutex::new(LiveState::default())),
            color: false,
        }
    }

    /// Print a single notice line to stdout (no colour, no ANSI).
    pub fn print_notice(&self, text: &str) {
        println!("{text}");
    }

    /// Print the run's branding header (`🦊 Ralphy - vX`) at start-up, seeded by a
    /// stable per-run `seed` (the derived run title), so the face is identical to the
    /// Telegram card for the run and varies across runs. Routed through
    /// `MultiProgress` when styled so it sits cleanly above the live region; bold-cyan
    /// on a colour TTY, plain otherwise.
    pub fn print_header(&self, seed: &str) {
        let header = crate::runstate::ralphy_header(seed);
        if self.color {
            let _ = self
                .multi
                .println(Style::new().cyan().bold().apply_to(&header).to_string());
        } else {
            println!("{header}");
        }
    }

    /// Print the start-up info line (project · branch · repo URL) under the header.
    /// Routed through `MultiProgress` when styled so it sits above the live region.
    pub fn print_info_line(&self, project: &str, branch: Option<&str>, url: Option<&str>) {
        let opts = RenderOpts {
            color: self.color,
            emoji: self.color,
        };
        let line = render_info_line(project, branch, url, opts);
        if self.color {
            let _ = self.multi.println(line);
        } else {
            println!("{line}");
        }
    }

    /// Print the end-of-run totals panel to stdout, coloured when the handle is
    /// styled. Called after `finalize` has cleared the live region.
    pub fn print_panel(&self, data: &PanelData) {
        let opts = RenderOpts {
            color: self.color,
            emoji: self.color,
        };
        for line in render_totals_panel(data, opts) {
            println!("{line}");
        }
    }

    /// Flush the queue bar to `N/N` (covering a trailing infeasible/dry-run issue
    /// with no following event) and clear the live region. No-op off a colour TTY.
    pub fn finalize(&self) {
        if !self.color {
            return;
        }
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(q) = s.queue.as_mut() {
            q.finish();
        }
        let label = s.queue.as_ref().map(|q| q.bar_label());
        if let (Some(bar), Some(label)) = (s.queue_bar.as_ref(), label) {
            bar.set_message(label);
        }
        if let Some(bar) = s.active_bar.take() {
            bar.finish_and_clear();
        }
        if let Some(bar) = s.queue_bar.take() {
            bar.finish_and_clear();
        }
        let _ = self.multi.clear();
    }
}

impl<S: Subscriber> Layer<S> for Presenter {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = EventFields {
            level: *event.metadata().level(),
            ..EventFields::default()
        };
        event.record(&mut fields);
        let Some(run_event) =
            event_to_runevent(event.metadata().target(), &fields.message, &fields)
        else {
            return;
        };
        self.apply(run_event);
    }
}
#[cfg(test)]
mod tests;
