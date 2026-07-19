//! The console presenter: a `tracing_subscriber::Layer` that consumes the events
//! the core and adapters already emit and renders the run's lifecycle as styled,
//! local-timestamped lines. The entire UI lives here (ADR-0006); the core stays a
//! queue engine that happens to log.
//!
//! The seam is thin: `on_event` calls `runstate::event_to_runevent` to decode the
//! raw tracing event into a [`RunEvent`], then hands it to a dedicated render thread
//! over a channel. ALL terminal I/O lives on that thread ([`Renderer`]), so a stalled
//! console write (e.g. Windows QuickEdit text selection pausing output) can only
//! freeze the UI, never the run thread that emitted the log line. The renderer owns
//! the side effects — timestamps, per-issue duration, and writing through
//! `indicatif`'s `MultiProgress` so warn/error lines never corrupt live output.

use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Local;
use console::Style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use super::render::{meter_for, pick, render_active_line, render_line, sleep_label, LineExtra};
use super::{
    active_phase, queue_bar_label, render_info_line, render_totals_panel, PanelData, RenderOpts,
};
use crate::pricing::PriceTable;
use crate::runstate::{event_to_runevent, EventFields, RunEvent, RunState};

/// The mutable live-region state behind the presenter's single `Mutex`: the folded
/// run, the active issue's wall-clock anchor, and the two `indicatif` bars (only
/// present on a colour TTY). Shared with the [`PresenterHandle`] via `Arc` so `main`
/// can flush and clear the region after the queue returns.
///
/// Every rendered fact is derived from `run` (ADR-0007 D6 amendment #223) — the
/// console keeps no reducer of its own. `active_start` is the one exception: no
/// event carries a wall clock, so the per-issue `Instant` stays presenter-local.
#[derive(Default)]
struct LiveState {
    run: RunState,
    /// `(issue number, start instant)` for the issue currently on the active line.
    active_start: Option<(u64, Instant)>,
    queue_bar: Option<ProgressBar>,
    active_bar: Option<ProgressBar>,
}

impl LiveState {
    /// The elapsed wall clock of `number`, when it is the issue the active line is
    /// timing. `None` for any other issue (a stale or superseded anchor).
    fn elapsed_of(&self, number: u64) -> Option<Duration> {
        self.active_start
            .filter(|(n, _)| *n == number)
            .map(|(_, t)| t.elapsed())
    }
}

/// The console presenter: a `tracing` Layer that renders the run's lifecycle. Its
/// `on_event` only decodes and forwards; the drawing runs on a separate [`Renderer`]
/// thread, so a wedged console write never blocks the run.
pub struct Presenter {
    /// Hands each decoded event to the render thread. The `Mutex` exists only so the
    /// `Layer` stays `Sync`; it is held for a single non-blocking `send`, never across
    /// terminal I/O — that is the freeze fix.
    tx: Mutex<Sender<PresenterMsg>>,
    /// Shared live region, handed to the [`PresenterHandle`] so teardown can clear it.
    state: Arc<Mutex<LiveState>>,
    multi: MultiProgress,
    color: bool,
}

/// A message to the render thread: an event to draw, or a flush barrier teardown uses
/// to drain every pending scroll line before it clears the live region.
enum PresenterMsg {
    Event(RunEvent),
    Flush(SyncSender<()>),
}

/// The render half of the presenter: owns all terminal I/O and the live state, and
/// runs on its own thread so a stalled console write (e.g. Windows QuickEdit text
/// selection pausing output) freezes only the UI, never the run.
struct Renderer {
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
        let state = Arc::new(Mutex::new(LiveState::default()));
        let multi = MultiProgress::new();
        let (tx, rx) = mpsc::channel();
        let renderer = Renderer {
            multi: multi.clone(),
            state: Arc::clone(&state),
            opts: RenderOpts {
                color: styled,
                emoji: styled,
            },
            price: PriceTable::load(),
        };
        // ALL terminal I/O runs on this one thread: it applies each event and, on a
        // styled TTY, repaints the active line every second so the elapsed clock keeps
        // advancing during quiet multi-minute stretches that emit no events. Because
        // `on_event` only hands work here (it never draws), a stalled console write
        // freezes the UI, not the run. The thread ends when the last `Sender` (this
        // `Presenter` and any `PresenterHandle`) drops.
        std::thread::spawn(move || renderer.run(rx));
        Presenter {
            tx: Mutex::new(tx),
            state,
            multi,
            color: styled,
        }
    }

    /// A teardown handle over the shared live region. `init_tracing` hands this to
    /// `run_cmd` so the queue bar is flushed to `N/N` and the live region cleared
    /// before the summary prints (ADR-0006: the presenter owns teardown).
    pub fn handle(&self) -> PresenterHandle {
        PresenterHandle {
            multi: self.multi.clone(),
            state: Arc::clone(&self.state),
            color: self.color,
            flush: Some(self.tx.lock().unwrap_or_else(|e| e.into_inner()).clone()),
            edge: Arc::new(Mutex::new(crate::ui::EdgeNoticeState::default())),
        }
    }
}

impl Renderer {
    /// The render loop: apply each event as it arrives, repaint the clock on the idle
    /// tick, and answer a [`PresenterMsg::Flush`] once everything already queued is
    /// drained (so teardown clears the region only after the last scroll line is on
    /// screen). Ends when every `Sender` has dropped.
    fn run(self, rx: Receiver<PresenterMsg>) {
        loop {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(PresenterMsg::Event(event)) => self.apply(event),
                Ok(PresenterMsg::Flush(ack)) => {
                    // Drain everything already queued so no scroll line is lost before
                    // the region is cleared; a superseded flush just gets acked too.
                    while let Ok(msg) = rx.try_recv() {
                        match msg {
                            PresenterMsg::Event(event) => self.apply(event),
                            PresenterMsg::Flush(prev) => {
                                let _ = prev.send(());
                            }
                        }
                    }
                    let _ = ack.send(());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Advance the elapsed clock between events (ADR-0006 D4). No-op off
                    // a colour TTY or when no active bar exists.
                    if self.opts.color {
                        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        repaint_active_bar(&s, self.opts);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
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
    /// the event closes the active issue. The fold runs FIRST and every rendered
    /// fact is read back from it; only the wall clock is presenter-local. The
    /// live-region (`indicatif`) calls are guarded behind `self.opts.color`, so
    /// `--verbose`/non-TTY draw nothing.
    fn drive(&self, s: &mut LiveState, event: &RunEvent) -> LineExtra {
        s.run.apply(event.clone());
        match event {
            RunEvent::QueueBuilt { .. } => {
                if self.opts.color {
                    let bar = self.multi.add(ProgressBar::new_spinner());
                    bar.set_style(ProgressStyle::with_template("{msg}").expect("static template"));
                    bar.set_message(queue_bar_label(&s.run, self.opts));
                    s.queue_bar = Some(bar);
                }
                LineExtra::default()
            }
            RunEvent::IssueStarted { number, .. } => {
                // The fold already superseded a still-non-terminal prior issue, so
                // the bar may have advanced.
                s.active_start = Some((*number, Instant::now()));
                self.refresh_queue_bar(s);
                self.refresh_active_bar(s);
                LineExtra::default()
            }
            RunEvent::Planning { .. } | RunEvent::Executing { .. } => {
                // Live-region only: the fold carries the planner's/executor's
                // display model, effort and budget onto the active entry.
                self.refresh_active_bar(s);
                LineExtra::default()
            }
            RunEvent::PlanWritten { usage, .. } => {
                // The scroll line shows the planning meter + elapsed-so-far; the
                // fold stashed the planning usage for the eventual `done` line.
                let meter = Some(meter_for(&self.price, None, usage));
                let extra = match s.run.active_issue() {
                    Some(e) => LineExtra {
                        duration: s.elapsed_of(e.number),
                        model: e.model.clone(),
                        effort: e.effort.clone(),
                        meter,
                    },
                    None => LineExtra {
                        meter,
                        ..Default::default()
                    },
                };
                // A zero-step plan is terminal (`infeasible`), so the fold just
                // advanced the bar and the active line has nothing left to tick —
                // leaving the spinner up would freeze a stale clock on screen.
                self.settle_if_terminal(s);
                extra
            }
            // A bundle verdict is terminal on arrival, exactly like an infeasible
            // plan: advance the bar and take the active line down.
            RunEvent::NeedsSplit { .. } => {
                self.settle_if_terminal(s);
                LineExtra::default()
            }
            RunEvent::IssueClosed { number, usage, .. } => {
                // The `done` line shows the issue total (plan + execute) and prices
                // each phase's model: combine the planning usage the fold stashed
                // with this execution usage.
                let extra = match s.run.active_issue().filter(|e| e.number == *number) {
                    Some(e) => LineExtra {
                        duration: s.elapsed_of(e.number),
                        model: e.model.clone(),
                        effort: e.effort.clone(),
                        meter: Some(meter_for(&self.price, e.plan_usage.as_ref(), usage)),
                    },
                    None => LineExtra::default(),
                };
                self.close_active(s);
                extra
            }
            RunEvent::NonGreen { number, .. }
            | RunEvent::Skipped { number, .. }
            | RunEvent::HumanBlocked { number, .. } => {
                let duration = s
                    .run
                    .active_issue()
                    .filter(|e| e.number == *number)
                    .and_then(|e| s.elapsed_of(e.number));
                self.close_active(s);
                LineExtra {
                    duration,
                    ..Default::default()
                }
            }
            RunEvent::SleepStarted { .. } => {
                if let Some(bar) = s.active_bar.take() {
                    bar.finish_and_clear();
                }
                self.refresh_queue_bar(s);
                LineExtra::default()
            }
            RunEvent::SleepEnded => {
                self.refresh_queue_bar(s);
                self.refresh_active_bar(s);
                LineExtra::default()
            }
            // The API-degraded transitions swap the active spinner's label for a
            // retry indicator immediately (unthrottled live region), unlike the
            // ~60s Telegram card refresh (issue #149). Live-region only — the
            // scroll line is drawn by `render_line`.
            RunEvent::ApiDegraded | RunEvent::ApiRecovered => {
                self.refresh_active_bar(s);
                LineExtra::default()
            }
            _ => LineExtra::default(),
        }
    }

    /// Close the active line when the fold just made the active issue terminal
    /// WITHOUT a lifecycle event of its own (`plan written` with zero steps, `needs
    /// split`). A no-op while the issue is still planning or executing.
    fn settle_if_terminal(&self, s: &mut LiveState) {
        let terminal = s
            .run
            .active_issue()
            .is_some_and(|e| active_phase(&e.status).is_none());
        if terminal {
            self.close_active(s);
        }
    }

    /// The active issue reached a terminal status: drop its wall-clock anchor, take
    /// down the spinner, and repaint the queue bar the fold just advanced.
    fn close_active(&self, s: &mut LiveState) {
        s.active_start = None;
        self.refresh_queue_bar(s);
        if let Some(bar) = s.active_bar.take() {
            bar.finish_and_clear();
        }
    }

    /// Repaint the queue bar's label from the folded state. No-op off a colour TTY.
    fn refresh_queue_bar(&self, s: &LiveState) {
        if !self.opts.color {
            return;
        }
        if let Some(bar) = s.queue_bar.as_ref() {
            let msg = match s.run.sleep.as_ref() {
                Some(sleep) => sleep_label(&sleep.reset, self.opts),
                None => queue_bar_label(&s.run, self.opts),
            };
            bar.set_message(msg);
        }
    }

    /// Repaint (creating on first use) the self-ticking active-issue spinner from the
    /// folded active entry. No-op off a colour TTY or when no issue is in a
    /// non-terminal phase. The message itself is rendered by [`repaint_active_bar`],
    /// shared with the per-second clock ticker.
    fn refresh_active_bar(&self, s: &mut LiveState) {
        let live = s
            .run
            .active_issue()
            .is_some_and(|e| active_phase(&e.status).is_some());
        if !self.opts.color || !live {
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
    let (Some(entry), Some(bar)) = (s.run.active_issue(), s.active_bar.as_ref()) else {
        return;
    };
    let Some(phase) = active_phase(&entry.status) else {
        return;
    };
    let line = render_active_line(
        phase,
        entry.number,
        &entry.title,
        entry.model.as_deref(),
        entry.effort.as_deref(),
        s.elapsed_of(entry.number).unwrap_or_default(),
        entry.budget_min,
        opts,
    );
    // API-degraded: prefix the spinner message with a retry indicator so the
    // operator sees the child is retrying, not stalled (issue #149).
    let msg = if s.run.degraded {
        format!("{} {line}", pick("🔄", "[api-retry]", opts.emoji))
    } else {
        line
    };
    bar.set_message(msg);
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
    /// Sender to the render thread so [`finalize`](Self::finalize) can flush the last
    /// scroll lines before clearing the region. `None` on the plain / banner path.
    flush: Option<Sender<PresenterMsg>>,
    /// The run-border notice folded off the bus (#222), shared with
    /// [`crate::ui::EdgeNoticeLayer`]. Detached (and thus always empty) unless
    /// `init_tracing` installed the layer.
    edge: Arc<Mutex<crate::ui::EdgeNoticeState>>,
}

impl PresenterHandle {
    /// A plain (colour off, no bars) handle for the `--verbose` / raw-stderr path.
    /// `finalize` is a no-op; `print_panel` produces uncoloured lines.
    pub fn plain() -> PresenterHandle {
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
    pub fn print_edge_notice(&self) {
        let notice = match self.edge.lock() {
            Ok(mut g) => g.take(),
            Err(e) => e.into_inner().take(),
        };
        if let Some(text) = notice {
            println!("{text}");
        }
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
        let _ = self.multi.clear();
    }

    /// Acquire the live state without blocking teardown forever: on a wedged render
    /// thread (holding the lock during a stalled console write) return `None` after
    /// `budget` instead of parking. Recovers from a poisoned lock.
    fn lock_state_bounded(&self, budget: Duration) -> Option<std::sync::MutexGuard<'_, LiveState>> {
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
        // Off the run path: hand the event to the render thread. The `Mutex` guards
        // only the `Sender` (never terminal I/O), so a wedged console can't block the
        // thread that emitted this log line. A closed channel (render thread gone at
        // shutdown) drops the line rather than erroring.
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(PresenterMsg::Event(run_event));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runstate::{IssueStatus, UsageLite};

    /// A plain (colour off, so no `indicatif` I/O) renderer over a fresh live state —
    /// the seam that lets `drive` be driven directly.
    fn plain_renderer() -> (Renderer, LiveState) {
        let renderer = Renderer {
            multi: MultiProgress::new(),
            state: Arc::new(Mutex::new(LiveState::default())),
            opts: RenderOpts {
                color: false,
                emoji: true,
            },
            price: PriceTable::load(),
        };
        (renderer, LiveState::default())
    }

    fn usage(input: u64, output: u64) -> UsageLite {
        UsageLite {
            input,
            cache_read: 0,
            cache_creation: 0,
            output,
            model: Some("claude-opus-4".into()),
        }
    }

    /// `drive` must derive the `done` line's whole tail from the fold: the elapsed
    /// clock from its own anchor, the model/effort from the folded entry, and the
    /// meter from the stashed PLAN usage plus this execution usage. This is the
    /// contract the deleted `ActiveIssue` used to carry, and nothing else pins it.
    #[test]
    fn drive_derives_the_done_line_extra_from_the_fold() {
        let (r, mut s) = plain_renderer();
        r.drive(
            &mut s,
            &RunEvent::IssueStarted {
                number: 7,
                title: "t".into(),
            },
        );
        r.drive(
            &mut s,
            &RunEvent::PlanWritten {
                number: 7,
                open_steps: 3,
                usage: usage(100, 10),
                steps: vec![],
            },
        );
        r.drive(
            &mut s,
            &RunEvent::Executing {
                number: 0,
                budget_min: 45,
                model: "claude-opus-4".into(),
                effort: Some("medium".into()),
            },
        );
        let extra = r.drive(
            &mut s,
            &RunEvent::IssueClosed {
                number: 7,
                tokens: 0,
                usage: usage(200, 20),
            },
        );
        assert_eq!(extra.model.as_deref(), Some("claude-opus-4"));
        assert_eq!(extra.effort.as_deref(), Some("medium"));
        assert!(extra.duration.is_some(), "the anchor keys issue 7");
        let meter = extra.meter.expect("a plan+exec meter");
        assert_eq!(meter.usage.input, 300, "plan (100) + exec (200) input");
        assert_eq!(meter.usage.output, 30, "plan (10) + exec (20) output");
        // The issue is closed: the anchor is dropped so no later line reuses it.
        assert!(s.active_start.is_none());
    }

    /// A `done` for an issue that is not the active one carries no derived tail —
    /// the old `filter(|a| a.number == *number)` guard, kept.
    #[test]
    fn drive_done_for_a_foreign_issue_carries_no_derived_tail() {
        let (r, mut s) = plain_renderer();
        r.drive(
            &mut s,
            &RunEvent::IssueStarted {
                number: 7,
                title: "t".into(),
            },
        );
        let extra = r.drive(
            &mut s,
            &RunEvent::IssueClosed {
                number: 99,
                tokens: 0,
                usage: UsageLite::default(),
            },
        );
        assert!(extra.duration.is_none());
        assert!(extra.model.is_none());
        assert!(extra.meter.is_none());
    }

    /// A zero-step plan is terminal on arrival: `drive` must settle the active line
    /// then and there, or the per-second ticker repaints a frozen clock for an issue
    /// the run has already left behind.
    #[test]
    fn drive_settles_the_active_line_on_a_terminal_plan() {
        let (r, mut s) = plain_renderer();
        r.drive(
            &mut s,
            &RunEvent::IssueStarted {
                number: 7,
                title: "t".into(),
            },
        );
        assert!(s.active_start.is_some(), "the anchor is set while planning");
        r.drive(
            &mut s,
            &RunEvent::PlanWritten {
                number: 7,
                open_steps: 0,
                usage: UsageLite::default(),
                steps: vec![],
            },
        );
        assert_eq!(s.run.issues[0].status, IssueStatus::Infeasible);
        assert!(
            s.active_start.is_none(),
            "an infeasible plan takes the active line down"
        );

        // A bundle verdict is the same shape and must settle too.
        let (r2, mut s2) = plain_renderer();
        r2.drive(
            &mut s2,
            &RunEvent::IssueStarted {
                number: 8,
                title: "b".into(),
            },
        );
        r2.drive(&mut s2, &RunEvent::NeedsSplit { number: 8 });
        assert_eq!(s2.run.issues[0].status, IssueStatus::NeedsSplit);
        assert!(s2.active_start.is_none());
    }

    /// A usage-limit sleep keeps the issue's wall clock running (the old presenter
    /// kept `ActiveIssue` across the sleep) — only the spinner comes down.
    #[test]
    fn drive_sleep_keeps_the_wall_clock_anchor() {
        let (r, mut s) = plain_renderer();
        r.drive(
            &mut s,
            &RunEvent::IssueStarted {
                number: 7,
                title: "t".into(),
            },
        );
        r.drive(
            &mut s,
            &RunEvent::SleepStarted {
                reset: "14:30".into(),
                target_epoch: 1_700_000_000,
            },
        );
        assert_eq!(s.active_start.map(|(n, _)| n), Some(7));
        r.drive(&mut s, &RunEvent::SleepEnded);
        assert_eq!(s.active_start.map(|(n, _)| n), Some(7));
    }

    /// The enqueue that `on_event` performs must never block the run thread, even if
    /// the render thread is wedged in a stalled console write and drains nothing — the
    /// freeze this whole design fixes. An unconsumed channel models the wedged renderer.
    #[test]
    fn enqueue_is_off_the_run_path_even_with_a_stalled_renderer() {
        let (tx, _rx) = mpsc::channel::<PresenterMsg>();
        let start = Instant::now();
        for _ in 0..1000 {
            tx.send(PresenterMsg::Event(RunEvent::SleepEnded))
                .expect("send never blocks");
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(50),
            "the on_event enqueue must be off the run path, took {elapsed:?}"
        );
    }

    /// Teardown must not hang if the render thread holds the state lock (wedged mid-draw
    /// in a stalled write): the bounded acquire returns `None` after its budget.
    #[test]
    fn finalize_lock_is_bounded_when_state_is_held() {
        let handle = PresenterHandle {
            multi: MultiProgress::new(),
            state: Arc::new(Mutex::new(LiveState::default())),
            color: true,
            flush: None,
            edge: Arc::new(Mutex::new(crate::ui::EdgeNoticeState::default())),
        };
        let _held = handle.state.lock().expect("hold the state lock");
        let start = Instant::now();
        let got = handle.lock_state_bounded(Duration::from_millis(100));
        let elapsed = start.elapsed();
        assert!(got.is_none(), "a held lock must not be acquired");
        assert!(
            elapsed < Duration::from_secs(1),
            "the bounded acquire must give up near its budget, took {elapsed:?}"
        );
    }
}
