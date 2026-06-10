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

use chrono::{DateTime, Local};
use console::Style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::runstate::{event_to_runevent, EventFields, RunEvent, SkipKind};

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
    fn label(self) -> &'static str {
        match self {
            FinishOutcome::Done => "done",
            FinishOutcome::Blocked => "blocked",
            FinishOutcome::Timeout => "timeout",
            FinishOutcome::Stuck => "stuck",
            FinishOutcome::Limit => "limit",
        }
    }

    /// `(emoji, ascii fallback, semantic colour)` — the ADR-0006 D4 icon table.
    fn glyph(self) -> (&'static str, &'static str, Style) {
        match self {
            FinishOutcome::Done => ("✅", "[ok]", Style::new().green()),
            FinishOutcome::Blocked => ("⛔", "[blocked]", Style::new().red()),
            FinishOutcome::Timeout => ("⏱️", "[timeout]", Style::new().yellow()),
            FinishOutcome::Stuck => ("🪨", "[stuck]", Style::new().red()),
            FinishOutcome::Limit => ("🌙", "[limit]", Style::new().yellow()),
        }
    }
}

/// Map the `?outcome` Debug string off `non-green — stopping run` to a
/// [`FinishOutcome`]. An unrecognised non-green outcome is treated as `Stuck`
/// rather than dropped, so the run never finishes line-less.
fn parse_outcome(debug: Option<&str>) -> FinishOutcome {
    match debug {
        Some(s) if s.starts_with("Done") => FinishOutcome::Done,
        Some(s) if s.starts_with("Blocked") => FinishOutcome::Blocked,
        Some(s) if s.starts_with("Timeout") => FinishOutcome::Timeout,
        Some(s) if s.starts_with("Limit") => FinishOutcome::Limit,
        _ => FinishOutcome::Stuck,
    }
}

/// Label for a skipped issue: a UI-local string keyed off `runstate::SkipKind`.
fn skip_label(kind: SkipKind) -> &'static str {
    match kind {
        SkipKind::BlockedBy => "skipped (blocked)",
        SkipKind::StopBefore => "skipped (stop-before)",
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
}

impl QueueState {
    /// Build from the `queue built` `count` and parsed `order`.
    pub fn built(count: u64, order: Vec<u64>) -> Self {
        QueueState {
            total: count,
            pending: order,
            completed: 0,
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

    /// Render `▰▰▰▱▱▱ 3/6 (pending #4 #5 #6)`. ANSI-free by construction.
    pub fn bar_label(&self) -> String {
        let done = self.completed.min(self.total) as usize;
        let left = (self.total.saturating_sub(self.completed)) as usize;
        let filled = "▰".repeat(done);
        let empty = "▱".repeat(left);
        let pending = if self.pending.is_empty() {
            String::new()
        } else {
            let nums: Vec<String> = self.pending.iter().map(|n| format!("#{n}")).collect();
            format!(" (pending {})", nums.join(" "))
        };
        format!("{filled}{empty} {}/{}{pending}", self.completed, self.total)
    }
}

/// Render the live active-issue line: phase icon · `#n` title · model · `elapsed`
/// (or `elapsed / budget`). Pure over its inputs; the emoji/ASCII and colour
/// choice come from `opts`. The non-colour path emits no ANSI byte.
fn render_active_line(
    phase: Phase,
    number: u64,
    title: &str,
    model: Option<&str>,
    elapsed: Duration,
    budget_min: Option<u64>,
    opts: RenderOpts,
) -> String {
    let icon = match phase {
        Phase::Planning => pick("🧠", "[plan]", opts.emoji),
        Phase::Executing => pick("⚙️", "[exec]", opts.emoji),
    };
    let mut parts: Vec<String> = vec![format!("{icon} #{number} {title}")];
    if let Some(m) = model {
        parts.push(if opts.color {
            Style::new().cyan().apply_to(m).to_string()
        } else {
            m.to_string()
        });
    }
    let clock = match budget_min {
        Some(b) => format!(
            "{} / {}",
            fmt_clock(elapsed),
            fmt_clock(Duration::from_secs(b * 60))
        ),
        None => fmt_clock(elapsed),
    };
    parts.push(if opts.color {
        Style::new().dim().apply_to(clock).to_string()
    } else {
        clock
    });
    parts.join(" · ")
}

/// `MM:SS` clock form (minutes may exceed 59), e.g. `12:43`, `45:00`. The
/// active-line/budget form; distinct from [`fmt_duration`]'s `2m13s` finished-line
/// form.
fn fmt_clock(d: Duration) -> String {
    let secs = d.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// How a line is rendered: whether ANSI colour and emoji are available. The
/// non-TTY / `NO_COLOR` path sets both `false`, guaranteeing no ANSI ever reaches
/// a redirected file (ADR-0006 D3).
#[derive(Debug, Clone, Copy)]
pub struct RenderOpts {
    pub color: bool,
    pub emoji: bool,
}

/// UI-local mirror of `BranchMode` — the panel renderer never depends on a core type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelBranchMode {
    New,
    Current,
}

/// UI-local mirror of `StopReason`, with `outcome` pre-formatted as a string so the
/// panel never imports a core enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelStop {
    Deadline,
    NonGreen { number: u64, outcome: String },
    StopBefore { number: u64 },
    Limit { number: u64, reset: Option<String> },
}

/// Input data for [`render_totals_panel`]. Derived from `QueueReport` in `main.rs`
/// and passed to `PresenterHandle::print_panel`.
#[derive(Debug, Clone)]
pub struct PanelData {
    pub branch: String,
    pub orig_branch: String,
    pub done: u64,
    pub blocked: u64,
    pub skipped: u64,
    pub commits: usize,
    pub stop: Option<PanelStop>,
    pub branch_mode: PanelBranchMode,
    pub dry_run: bool,
}

/// Render a [`RunEvent`] to a single line, or `None` for live-region-only events.
/// The local timestamp and the outcome glyph are always present on a surfaced
/// line; colour is applied only when `opts.color` is set.
fn render_line(
    event: &RunEvent,
    ts: &DateTime<Local>,
    duration: Option<Duration>,
    opts: RenderOpts,
) -> Option<String> {
    let ts_str = ts.format("%Y-%m-%d %H:%M:%S").to_string();
    let dur = duration
        .map(|d| format!(" ({})", fmt_duration(d)))
        .unwrap_or_default();

    let (glyph, style, body) = match event {
        RunEvent::QueueBuilt { count, .. } => (
            pick("📋", "[queue]", opts.emoji),
            Style::new().cyan(),
            format!("queue built: {count} issue(s)"),
        ),
        // Live-region only: the active line carries the execution phase; no
        // permanent scroll-up line is drawn for it.
        RunEvent::Executing { .. } => return None,
        RunEvent::IssueStarted { number, title } => (
            pick("🧠", "[plan]", opts.emoji),
            Style::new().cyan(),
            format!("#{number} {title} — planning"),
        ),
        RunEvent::PlanWritten { number, open_steps } => (
            pick("📝", "[plan]", opts.emoji),
            Style::new().cyan(),
            format!("#{number} plan written ({open_steps} step(s))"),
        ),
        RunEvent::IssueClosed { number } => {
            let outcome = FinishOutcome::Done;
            let (emoji, ascii, style) = outcome.glyph();
            (
                pick(emoji, ascii, opts.emoji),
                style,
                format!("#{number} {}{dur}", outcome.label()),
            )
        }
        RunEvent::NonGreen { number, outcome } => {
            let fo = parse_outcome(Some(outcome));
            let (emoji, ascii, style) = fo.glyph();
            (
                pick(emoji, ascii, opts.emoji),
                style,
                format!("#{number} {}{dur}", fo.label()),
            )
        }
        RunEvent::Skipped { number, kind } => (
            pick("⏭️", "[skip]", opts.emoji),
            Style::new().dim(),
            format!("#{number} {}{dur}", skip_label(*kind)),
        ),
        RunEvent::Notice { level, message } => {
            if *level == Level::ERROR {
                (
                    pick("💥", "[error]", opts.emoji),
                    Style::new().red(),
                    message.clone(),
                )
            } else {
                (
                    pick("⚠️", "[warn]", opts.emoji),
                    Style::new().yellow(),
                    message.clone(),
                )
            }
        }
        RunEvent::SleepStarted { reset, .. } => (
            pick("🌙", "[limit]", opts.emoji),
            Style::new().yellow(),
            format!("usage limit — sleeping until {reset}"),
        ),
        RunEvent::SleepEnded => (
            pick("🌙", "[limit]", opts.emoji),
            Style::new().yellow(),
            "usage limit reset — resuming".to_string(),
        ),
        RunEvent::DeadlinePassed { number } => (
            pick("⏱️", "[timeout]", opts.emoji),
            Style::new().yellow(),
            format!("deadline reached before #{number}"),
        ),
    };

    Some(if opts.color {
        format!(
            "{}  {} {}",
            Style::new().dim().apply_to(&ts_str),
            style.apply_to(glyph),
            style.apply_to(body),
        )
    } else {
        format!("{ts_str}  {glyph} {body}")
    })
}

/// Render a [`RunEvent`] to a plain, ANSI-free line (local timestamp + outcome
/// glyph + body). The non-TTY / `NO_COLOR` clean-line path; also the public seam
/// the unit tests assert against.
pub fn render_plain_line(
    event: &RunEvent,
    ts: &DateTime<Local>,
    duration: Option<Duration>,
) -> Option<String> {
    render_line(
        event,
        ts,
        duration,
        RenderOpts {
            color: false,
            emoji: true,
        },
    )
}

/// Render the end-of-run totals panel as a `Vec<String>` of lines ready to
/// `println!`. Produces: a counts line (`✅/⛔/⏭️`), a commits line, an optional
/// stop-reason line, a per-mode/dry-run closing-state line, and — only for `New`
/// mode when not `(dry_run && commits == 0)` — a `➜  git merge <branch>` next-step
/// line. ANSI colour is applied only when `opts.color`; the non-TTY path is
/// guaranteed to contain no `\u{1b}` byte.
pub fn render_totals_panel(data: &PanelData, opts: RenderOpts) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    // Counts line
    let done_icon = pick("✅", "[ok]", opts.emoji);
    let blocked_icon = pick("⛔", "[blocked]", opts.emoji);
    let skipped_icon = pick("⏭️", "[skip]", opts.emoji);
    let done_part = format!("{done_icon} {} done", data.done);
    let blocked_part = format!("{blocked_icon} {} blocked", data.blocked);
    let skipped_part = format!("{skipped_icon} {} skipped", data.skipped);
    lines.push(if opts.color {
        format!(
            "{} · {} · {}",
            Style::new().green().apply_to(&done_part),
            Style::new().red().apply_to(&blocked_part),
            Style::new().dim().apply_to(&skipped_part),
        )
    } else {
        format!("{done_part} · {blocked_part} · {skipped_part}")
    });

    // Commits line
    let commits_raw = format!("{} commit(s) on '{}'", data.commits, data.branch);
    lines.push(if opts.color {
        Style::new().dim().apply_to(&commits_raw).to_string()
    } else {
        commits_raw
    });

    // Stop-reason line (reuses wording from the old main.rs match arms)
    if let Some(stop) = &data.stop {
        let stop_raw = match stop {
            PanelStop::Deadline => {
                "Stopped: run deadline reached (before the next issue, or a usage-limit reset landed past it).".to_string()
            }
            PanelStop::NonGreen { number, outcome } => {
                format!("Stopped: #{number} finished non-green ({outcome}). Branch handed back.")
            }
            PanelStop::StopBefore { number } => {
                format!("Stopped: stop-before label on #{number}. Remove the label and re-run to continue.")
            }
            PanelStop::Limit {
                number,
                reset: Some(t),
            } => {
                format!("Stopped: usage limit on #{number}. Reset ~{t}; re-run to continue (or it stalled with no progress).")
            }
            PanelStop::Limit {
                number,
                reset: None,
            } => {
                format!("Stopped: usage limit on #{number}. No parseable reset time; re-run after the limit clears.")
            }
        };
        lines.push(if opts.color {
            Style::new().yellow().apply_to(&stop_raw).to_string()
        } else {
            stop_raw
        });
    }

    // Per-mode/dry-run closing-state line
    let stopped = data.stop.is_some();
    let closing_raw = match data.branch_mode {
        PanelBranchMode::Current => {
            if data.dry_run {
                format!("DryRun on '{}': no commits made.", data.branch)
            } else if stopped {
                format!("Left repo on '{}' for inspection.", data.branch)
            } else {
                format!(
                    "Clean run: {} commit(s) added to '{}' in place.",
                    data.commits, data.branch
                )
            }
        }
        PanelBranchMode::New => {
            if data.dry_run {
                format!(
                    "DryRun: returned repo to '{}'; empty run branch removed.",
                    data.orig_branch
                )
            } else if stopped {
                format!("Left repo checked out on '{}' for inspection.", data.branch)
            } else {
                format!(
                    "Clean run: returned repo to '{}'. Run branch '{}' kept.",
                    data.orig_branch, data.branch
                )
            }
        }
    };
    lines.push(if opts.color {
        Style::new().dim().apply_to(&closing_raw).to_string()
    } else {
        closing_raw
    });

    // Next-step line: New mode only, absent when dry-run + zero commits
    if data.branch_mode == PanelBranchMode::New && !(data.dry_run && data.commits == 0) {
        let next_raw = format!("➜  git merge {}", data.branch);
        lines.push(if opts.color {
            Style::new().cyan().apply_to(&next_raw).to_string()
        } else {
            next_raw
        });
    }

    lines
}

/// Choose the emoji or its ASCII fallback.
fn pick(emoji: &'static str, ascii: &'static str, use_emoji: bool) -> &'static str {
    if use_emoji {
        emoji
    } else {
        ascii
    }
}

/// `13s` or `2m05s`.
fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
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
    budget_min: Option<u64>,
}

/// The mutable live-region state behind the presenter's single `Mutex`: the active
/// issue, the pure queue state, and the two `indicatif` bars (only present on a
/// colour TTY). Shared with the [`PresenterHandle`] via `Arc` so `main` can flush
/// and clear the region after the queue returns.
#[derive(Default)]
struct LiveState {
    active: Option<ActiveIssue>,
    queue: Option<QueueState>,
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
}

impl Presenter {
    /// Build a presenter, auto-detecting the terminal: styled (colour + emoji)
    /// on an attended TTY without `NO_COLOR`, else a plain clean-line renderer.
    pub fn new() -> Self {
        let is_tty = console::Term::stderr().is_term();
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let styled = is_tty && !no_color;
        Presenter {
            multi: MultiProgress::new(),
            state: Arc::new(Mutex::new(LiveState::default())),
            opts: RenderOpts {
                color: styled,
                emoji: styled,
            },
        }
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

        let duration = self.drive(&mut s, &event);

        // Styled lines on a colour TTY (routed through `MultiProgress` so they
        // never tear the live region); one clean, ANSI-free line per event
        // otherwise (the non-TTY / `NO_COLOR` path, ADR-0006 D3).
        let line = if self.opts.color {
            render_line(&event, &ts, duration, self.opts)
        } else {
            render_plain_line(&event, &ts, duration)
        };
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
    fn drive(&self, s: &mut LiveState, event: &RunEvent) -> Option<Duration> {
        match event {
            RunEvent::QueueBuilt { count, order } => {
                s.queue = Some(QueueState::built(*count, order.clone()));
                if self.opts.color {
                    let bar = self.multi.add(ProgressBar::new_spinner());
                    bar.set_style(ProgressStyle::with_template("{msg}").expect("static template"));
                    if let Some(q) = s.queue.as_ref() {
                        bar.set_message(q.bar_label());
                    }
                    s.queue_bar = Some(bar);
                }
                None
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
                    budget_min: None,
                });
                self.refresh_active_bar(s);
                None
            }
            RunEvent::Executing {
                model, budget_min, ..
            } => {
                // The event carries no number; it applies to the active issue.
                if let Some(a) = s.active.as_mut() {
                    a.phase = Phase::Executing;
                    a.model = Some(model.clone());
                    a.budget_min = Some(*budget_min);
                }
                self.refresh_active_bar(s);
                None
            }
            RunEvent::IssueClosed { number }
            | RunEvent::NonGreen { number, .. }
            | RunEvent::Skipped { number, .. } => {
                let d = s
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
                d
            }
            _ => None,
        }
    }

    /// Repaint the queue bar's label from the current [`QueueState`]. No-op off a
    /// colour TTY.
    fn refresh_queue_bar(&self, s: &LiveState) {
        if !self.opts.color {
            return;
        }
        if let (Some(bar), Some(q)) = (s.queue_bar.as_ref(), s.queue.as_ref()) {
            bar.set_message(q.bar_label());
        }
    }

    /// Repaint (creating on first use) the self-ticking active-issue spinner from
    /// the current [`ActiveIssue`]. No-op off a colour TTY.
    fn refresh_active_bar(&self, s: &mut LiveState) {
        if !self.opts.color {
            return;
        }
        let msg = match s.active.as_ref() {
            Some(a) => render_active_line(
                a.phase,
                a.number,
                &a.title,
                a.model.as_deref(),
                a.start.elapsed(),
                a.budget_min,
                self.opts,
            ),
            None => return,
        };
        let bar = s.active_bar.get_or_insert_with(|| {
            let b = self.multi.add(ProgressBar::new_spinner());
            b.set_style(
                ProgressStyle::with_template("{spinner:.cyan} {msg}").expect("static template"),
            );
            // Self-tick so the spinner keeps moving through quiet multi-minute
            // execution stretches that emit no events (ADR-0006 D4).
            b.enable_steady_tick(Duration::from_millis(120));
            b
        });
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
mod tests {
    use super::*;
    use crate::runstate::{RunEvent, SkipKind};
    use chrono::TimeZone;
    use tracing::Level;

    #[test]
    fn render_plain_finished_carries_timestamp_glyph_and_no_ansi() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
            .single()
            .unwrap();
        let event = RunEvent::IssueClosed { number: 30 };
        let line = render_plain_line(&event, &ts, Some(Duration::from_secs(133))).expect("a line");

        assert!(
            line.contains("2026-06-10 14:03:21"),
            "carries the local timestamp: {line}"
        );
        assert!(line.contains('✅'), "carries the outcome glyph: {line}");
        assert!(line.contains("#30"), "carries the issue number: {line}");
        assert!(line.contains("2m13s"), "carries the duration: {line}");
        assert!(
            !line.contains('\u{1b}'),
            "plain line has no ANSI escape byte: {line:?}"
        );
    }

    #[test]
    fn render_plain_executing_is_none() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
            .single()
            .unwrap();
        assert_eq!(
            render_plain_line(
                &RunEvent::Executing {
                    number: 0,
                    model: String::new(),
                    budget_min: 0,
                },
                &ts,
                None
            ),
            None
        );
    }

    #[test]
    fn render_plain_notice_shows_warn_and_error_glyphs() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
            .single()
            .unwrap();
        let warn_line = render_plain_line(
            &RunEvent::Notice {
                level: Level::WARN,
                message: "could not return to 'main'".to_string(),
            },
            &ts,
            None,
        )
        .expect("warn renders a line");
        assert!(warn_line.contains('⚠'), "warn glyph: {warn_line}");
        assert!(
            warn_line.contains("could not return to 'main'"),
            "warn message: {warn_line}"
        );

        let error_line = render_plain_line(
            &RunEvent::Notice {
                level: Level::ERROR,
                message: "boom".to_string(),
            },
            &ts,
            None,
        )
        .expect("error renders a line");
        assert!(error_line.contains('💥'), "error glyph: {error_line}");
        assert!(error_line.contains("boom"), "error message: {error_line}");
    }

    #[test]
    fn render_plain_sleep_started_ended_deadline_return_some_and_executing_none() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
            .single()
            .unwrap();

        let sleep_start = render_plain_line(
            &RunEvent::SleepStarted {
                reset: "15:30".to_string(),
                target_epoch: 1_000_000,
            },
            &ts,
            None,
        )
        .expect("SleepStarted renders a line");
        assert!(
            sleep_start.contains("usage limit"),
            "SleepStarted body: {sleep_start}"
        );
        assert!(
            sleep_start.contains("15:30"),
            "SleepStarted reset time: {sleep_start}"
        );

        let sleep_end =
            render_plain_line(&RunEvent::SleepEnded, &ts, None).expect("SleepEnded renders a line");
        assert!(
            sleep_end.contains("resuming"),
            "SleepEnded body: {sleep_end}"
        );

        let deadline = render_plain_line(&RunEvent::DeadlinePassed { number: 42 }, &ts, None)
            .expect("DeadlinePassed renders a line");
        assert!(
            deadline.contains("deadline"),
            "DeadlinePassed body: {deadline}"
        );
        assert!(
            deadline.contains("#42"),
            "DeadlinePassed number: {deadline}"
        );

        // Executing is live-region only — no permanent line.
        assert_eq!(
            render_plain_line(
                &RunEvent::Executing {
                    number: 0,
                    model: String::new(),
                    budget_min: 0,
                },
                &ts,
                None,
            ),
            None
        );
    }

    #[test]
    fn render_plain_skipped_shows_skip_label() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
            .single()
            .unwrap();
        let blocked = render_plain_line(
            &RunEvent::Skipped {
                number: 7,
                kind: SkipKind::BlockedBy,
            },
            &ts,
            None,
        )
        .expect("Skipped renders a line");
        assert!(blocked.contains("skipped (blocked)"), "{blocked}");

        let stop_before = render_plain_line(
            &RunEvent::Skipped {
                number: 8,
                kind: SkipKind::StopBefore,
            },
            &ts,
            None,
        )
        .expect("StopBefore renders a line");
        assert!(
            stop_before.contains("skipped (stop-before)"),
            "{stop_before}"
        );
    }

    #[test]
    fn fmt_duration_formats_minutes_and_seconds() {
        assert_eq!(fmt_duration(Duration::from_secs(13)), "13s");
        assert_eq!(fmt_duration(Duration::from_secs(133)), "2m13s");
        assert_eq!(fmt_duration(Duration::from_secs(120)), "2m00s");
    }

    #[test]
    fn queue_state_advances_through_all_terminal_outcomes_to_n_over_n() {
        // A queue of five issues, each leaving via a distinct terminal transition:
        // done, non-green, blocked, stop-before, and a superseded infeasible plan.
        let mut q = QueueState::built(5, vec![10, 11, 12, 13, 14]);
        assert_eq!(q.bar_label(), "▱▱▱▱▱ 0/5 (pending #10 #11 #12 #13 #14)");

        // done
        q.advance(10);
        // non-green (stopping run)
        q.advance(11);
        // blocked-by skip
        q.advance(12);
        // stop-before skip
        q.advance(13);
        assert_eq!(q.bar_label(), "▰▰▰▰▱ 4/5 (pending #14)");

        // #14 is an infeasible/dry-run plan: no terminal event, completed only when
        // a following `issue started` supersedes it.
        q.supersede(14);
        assert_eq!(q.completed, 5);
        assert_eq!(q.bar_label(), "▰▰▰▰▰ 5/5");

        // Idempotent: a stray repeat never over-counts past N/N.
        q.advance(14);
        assert_eq!(q.completed, 5);

        // `finish` is a safe flush even when already complete.
        q.finish();
        assert_eq!(q.bar_label(), "▰▰▰▰▰ 5/5");
    }

    #[test]
    fn queue_state_finish_flushes_trailing_issue_to_n_over_n() {
        // A trailing infeasible issue with no following `issue started`: only the
        // end-of-run `finish` flushes the bar to N/N.
        let mut q = QueueState::built(3, vec![1, 2, 3]);
        q.advance(1);
        q.advance(2);
        assert_eq!(q.bar_label(), "▰▰▱ 2/3 (pending #3)");
        q.finish();
        assert_eq!(q.bar_label(), "▰▰▰ 3/3");
    }

    #[test]
    fn render_active_line_executing_shows_icon_number_title_model_and_budget() {
        let opts = RenderOpts {
            color: false,
            emoji: true,
        };
        let line = render_active_line(
            Phase::Executing,
            31,
            "Console UI",
            Some("sonnet"),
            Duration::from_secs(12 * 60 + 43),
            Some(45),
            opts,
        );
        assert!(line.contains('⚙'), "executing phase icon: {line}");
        assert!(line.contains("#31"), "issue number: {line}");
        assert!(line.contains("Console UI"), "title: {line}");
        assert!(line.contains("sonnet"), "model: {line}");
        assert!(line.contains("12:43 / 45:00"), "elapsed / budget: {line}");
        assert!(!line.contains('\u{1b}'), "no ANSI byte: {line:?}");
    }

    #[test]
    fn render_active_line_planning_shows_brain_icon_and_no_budget() {
        let opts = RenderOpts {
            color: false,
            emoji: true,
        };
        let line = render_active_line(
            Phase::Planning,
            31,
            "Console UI",
            None,
            Duration::from_secs(12),
            None,
            opts,
        );
        assert!(line.contains('🧠'), "planning phase icon: {line}");
        assert!(line.contains("0:12"), "elapsed clock: {line}");
        assert!(
            !line.contains('/'),
            "no budget slash while planning: {line}"
        );
        assert!(!line.contains('\u{1b}'), "no ANSI byte: {line:?}");
    }

    #[test]
    fn render_active_line_no_colour_emits_no_ansi() {
        let opts = RenderOpts {
            color: false,
            emoji: false,
        };
        let line = render_active_line(
            Phase::Executing,
            31,
            "title",
            Some("opus"),
            Duration::from_secs(63),
            Some(45),
            opts,
        );
        assert!(line.contains("[exec]"), "ascii phase fallback: {line}");
        assert!(!line.contains('\u{1b}'), "no ANSI byte: {line:?}");
    }

    #[test]
    fn bar_label_no_colour_emits_no_ansi() {
        let mut q = QueueState::built(6, vec![1, 2, 3, 4, 5, 6]);
        q.advance(1);
        q.advance(2);
        q.advance(3);
        let label = q.bar_label();
        assert_eq!(label, "▰▰▰▱▱▱ 3/6 (pending #4 #5 #6)");
        assert!(!label.contains('\u{1b}'), "no ANSI byte: {label:?}");
    }

    #[test]
    fn fmt_clock_formats_mm_ss() {
        assert_eq!(fmt_clock(Duration::from_secs(12 * 60 + 43)), "12:43");
        assert_eq!(fmt_clock(Duration::from_secs(45 * 60)), "45:00");
        assert_eq!(fmt_clock(Duration::from_secs(5)), "0:05");
        assert_eq!(fmt_clock(Duration::from_secs(72 * 60 + 5)), "72:05");
    }

    fn panel_base() -> PanelData {
        PanelData {
            branch: "afk/run-20260610-120000".to_string(),
            orig_branch: "main".to_string(),
            done: 3,
            blocked: 1,
            skipped: 2,
            commits: 5,
            stop: None,
            branch_mode: PanelBranchMode::New,
            dry_run: false,
        }
    }

    #[test]
    fn render_totals_panel_counts_line_and_no_per_issue_relisting() {
        let opts = RenderOpts {
            color: false,
            emoji: true,
        };
        let lines = render_totals_panel(&panel_base(), opts);
        let all = lines.join("\n");

        // Counts line has the correct triad and numbers.
        assert!(lines[0].contains("✅ 3 done"), "done count: {}", lines[0]);
        assert!(
            lines[0].contains("⛔ 1 blocked"),
            "blocked count: {}",
            lines[0]
        );
        assert!(
            lines[0].contains("⏭️ 2 skipped"),
            "skipped count: {}",
            lines[0]
        );
        // No per-issue `#N:` re-listing — the old format was `  #N: Done`.
        assert!(!all.contains(": Done"), "no per-issue Done line: {all}");
        assert!(
            !all.contains(": Blocked"),
            "no per-issue Blocked line: {all}"
        );
        assert!(
            !all.contains(": Timeout"),
            "no per-issue Timeout line: {all}"
        );
    }

    #[test]
    fn render_totals_panel_git_merge_line_presence_rules() {
        let opts = RenderOpts {
            color: false,
            emoji: true,
        };

        // New + commits > 0: merge line present.
        let lines = render_totals_panel(&panel_base(), opts);
        let all = lines.join("\n");
        assert!(
            all.contains("git merge afk/run-20260610-120000"),
            "merge line present for New+commits: {all}"
        );

        // New + dry_run + 0 commits: no merge line.
        let dry_zero = PanelData {
            dry_run: true,
            commits: 0,
            ..panel_base()
        };
        let all2 = render_totals_panel(&dry_zero, opts).join("\n");
        assert!(
            !all2.contains("git merge"),
            "no merge line for New+dry_run+0-commits: {all2}"
        );

        // Current mode: no merge line regardless of commits.
        let current = PanelData {
            branch_mode: PanelBranchMode::Current,
            ..panel_base()
        };
        let all3 = render_totals_panel(&current, opts).join("\n");
        assert!(
            !all3.contains("git merge"),
            "no merge line for Current mode: {all3}"
        );
    }

    #[test]
    fn render_totals_panel_plain_no_ansi_and_stop_reason_present() {
        let opts = RenderOpts {
            color: false,
            emoji: true,
        };
        let data = PanelData {
            stop: Some(PanelStop::NonGreen {
                number: 42,
                outcome: "Blocked(\"reason\")".to_string(),
            }),
            ..panel_base()
        };
        let lines = render_totals_panel(&data, opts);
        let all = lines.join("\n");

        // No ANSI escape bytes on the plain path.
        assert!(!all.contains('\u{1b}'), "no ANSI in plain render: {all:?}");

        // Stop-reason line is present and references the issue.
        assert!(all.contains("Stopped:"), "stop-reason line present: {all}");
        assert!(all.contains("#42"), "issue number in stop line: {all}");

        // Done/blocked/skipped counts from the supplied PanelData match.
        assert!(all.contains("3 done"), "done count preserved: {all}");
        assert!(all.contains("1 blocked"), "blocked count preserved: {all}");
        assert!(all.contains("2 skipped"), "skipped count preserved: {all}");
    }
}
