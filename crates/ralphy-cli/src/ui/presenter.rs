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
    render_info_line, render_totals_panel, PanelData, Phase, QueueState, RenderOpts, UsageLite,
};
use crate::pricing::PriceTable;
use crate::runstate::{event_to_runevent, EventFields, RunEvent};

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
    /// The active child is in a sustained API-degraded state (issue #149): the
    /// active spinner shows a retry indicator until recovery. Live-region only.
    degraded: bool,
    queue_bar: Option<ProgressBar>,
    active_bar: Option<ProgressBar>,
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
            // The API-degraded transitions swap the active spinner's label for a
            // retry indicator immediately (unthrottled live region), unlike the
            // ~60s Telegram card refresh (issue #149). Live-region only — the
            // scroll line is drawn by `render_line`.
            RunEvent::ApiDegraded => {
                s.degraded = true;
                self.refresh_active_bar(s);
                LineExtra::default()
            }
            RunEvent::ApiRecovered => {
                s.degraded = false;
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
        let line = render_active_line(
            a.phase,
            a.number,
            &a.title,
            a.model.as_deref(),
            a.effort.as_deref(),
            a.start.elapsed(),
            a.budget_min,
            opts,
        );
        // API-degraded: prefix the spinner message with a retry indicator so the
        // operator sees the child is retrying, not stalled (issue #149).
        let msg = if s.degraded {
            format!("{} {line}", pick("🔄", "[api-retry]", opts.emoji))
        } else {
            line
        };
        bar.set_message(msg);
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
    /// Sender to the render thread so [`finalize`](Self::finalize) can flush the last
    /// scroll lines before clearing the region. `None` on the plain / banner path.
    flush: Option<Sender<PresenterMsg>>,
}

impl PresenterHandle {
    /// A plain (colour off, no bars) handle for the `--verbose` / raw-stderr path.
    /// `finalize` is a no-op; `print_panel`/`print_notice` produce uncoloured lines.
    pub fn plain() -> PresenterHandle {
        PresenterHandle {
            multi: MultiProgress::new(),
            state: Arc::new(Mutex::new(LiveState::default())),
            color: false,
            flush: None,
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
