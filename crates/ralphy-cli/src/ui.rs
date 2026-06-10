//! The console presenter: a `tracing_subscriber::Layer` that consumes the events
//! the core and adapters already emit and renders the run's lifecycle as styled,
//! local-timestamped lines. The entire UI lives here (ADR-0006); the core stays a
//! queue engine that happens to log.
//!
//! The seam is deliberately thin: a pure [`classify_event`] maps an event's
//! `(target, message, fields)` to a [`UiAction`], unit-tested in isolation in the
//! same style as the adapters' `classify_*` functions, so an event/UI drift fails
//! a test rather than silently breaking the display. The [`Presenter`] then owns
//! the side effects — timestamps, per-issue duration, and writing through
//! `indicatif`'s `MultiProgress` so `warn`/`error` lines never corrupt live output.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use console::Style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

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

/// Why an issue was skipped (not worked, not a run stop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipKind {
    BlockedBy,
    StopBefore,
}

impl SkipKind {
    fn label(self) -> &'static str {
        match self {
            SkipKind::BlockedBy => "skipped (blocked)",
            SkipKind::StopBefore => "skipped (stop-before)",
        }
    }
}

/// What the presenter should do with one event. The mapping from an event to a
/// `UiAction` is the entire consumed contract (ADR-0006 D1); everything the
/// presenter does is keyed off this enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    QueueBuilt {
        count: u64,
        /// The issue numbers in queue order, parsed from the `#a -> #b` string.
        order: Vec<u64>,
    },
    IssueStarted {
        number: u64,
        title: String,
    },
    /// The active issue moved from planning into execution; carries the resolved
    /// model and per-issue budget for the live active line. Live-region only — it
    /// renders no permanent line.
    Executing {
        number: u64,
        model: String,
        budget_min: u64,
    },
    PlanWritten {
        number: u64,
        open_steps: u64,
    },
    Finished {
        number: u64,
        outcome: FinishOutcome,
    },
    Skipped {
        number: u64,
        kind: SkipKind,
    },
    Warn {
        message: String,
    },
    Error {
        message: String,
    },
    /// An event the presenter does not surface (its full text still reaches
    /// `ralphy.log`).
    Ignore,
}

/// The typed fields extracted off one `tracing` event, plus its level. Populated
/// by the [`Visit`] impl below; consumed by [`classify_event`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFields {
    pub level: Level,
    pub message: String,
    pub number: Option<u64>,
    pub title: Option<String>,
    pub open_steps: Option<u64>,
    pub count: Option<u64>,
    pub outcome: Option<String>,
    pub order: Option<String>,
    pub model: Option<String>,
    pub budget_min: Option<u64>,
}

impl Default for EventFields {
    fn default() -> Self {
        EventFields {
            level: Level::INFO,
            message: String::new(),
            number: None,
            title: None,
            open_steps: None,
            count: None,
            outcome: None,
            order: None,
            model: None,
            budget_min: None,
        }
    }
}

impl Visit for EventFields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "number" => self.number = Some(value),
            "open_steps" => self.open_steps = Some(value),
            "count" => self.count = Some(value),
            "budget_min" => self.budget_min = Some(value),
            _ => {}
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "title" => self.title = Some(value.to_string()),
            "outcome" => self.outcome = Some(value.to_string()),
            "order" => self.order = Some(value.to_string()),
            "model" => self.model = Some(value.to_string()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // `%field` (Display) and `?field` (Debug) both arrive here; the message
        // literal also arrives as a `format_args!` debug value.
        let rendered = format!("{value:?}");
        match field.name() {
            "message" => self.message = rendered,
            "title" => self.title = Some(rendered),
            "outcome" => self.outcome = Some(rendered),
            "order" => self.order = Some(rendered),
            "model" => self.model = Some(rendered),
            _ => {}
        }
    }
}

/// Map an event's `(target, message, fields)` to a [`UiAction`]. Pure over its
/// inputs and unit-tested per lifecycle event so an event/UI drift fails a test
/// rather than silently breaking the display (ADR-0006 D1).
///
/// `target` is currently informational — the message + fields uniquely identify
/// every consumed event — but kept in the signature so a future disambiguation
/// (two crates, same message) has the discriminator on hand.
pub fn classify_event(target: &str, message: &str, fields: &EventFields) -> UiAction {
    let _ = target;

    // Level wins over message: any WARN/ERROR surfaces as a styled line so a
    // future warning can never silently vanish from the terminal (ADR-0006 D3).
    if fields.level == Level::ERROR {
        return UiAction::Error {
            message: message.to_string(),
        };
    }
    if fields.level == Level::WARN {
        return UiAction::Warn {
            message: message.to_string(),
        };
    }

    let number = fields.number.unwrap_or(0);
    match message {
        "queue built" => UiAction::QueueBuilt {
            count: fields.count.unwrap_or(0),
            order: parse_order(fields.order.as_deref()),
        },
        "issue started" => UiAction::IssueStarted {
            number,
            title: fields.title.clone().unwrap_or_default(),
        },
        // The adapter's execution events carry no issue number (the adapter never
        // receives it, ADR-0006 D2); the presenter applies this to whichever issue
        // is currently active. `number` is therefore 0 here.
        "executing with interactive claude over the PTY"
        | "executing with headless claude -p loop" => UiAction::Executing {
            number,
            model: fields.model.clone().unwrap_or_default(),
            budget_min: fields.budget_min.unwrap_or(0),
        },
        "plan written" => UiAction::PlanWritten {
            number,
            open_steps: fields.open_steps.unwrap_or(0),
        },
        "green — issue closed" => UiAction::Finished {
            number,
            outcome: FinishOutcome::Done,
        },
        "non-green — stopping run" => UiAction::Finished {
            number,
            outcome: parse_outcome(fields.outcome.as_deref()),
        },
        "blocked by open issue(s) — skipping" => UiAction::Skipped {
            number,
            kind: SkipKind::BlockedBy,
        },
        "stop-before label — halting run before this issue" => UiAction::Skipped {
            number,
            kind: SkipKind::StopBefore,
        },
        _ => UiAction::Ignore,
    }
}

/// Map the `?outcome` Debug string off `non-green — stopping run` to a
/// [`FinishOutcome`]. An unrecognised non-green outcome is treated as `Stuck`
/// rather than dropped, so the run never finishes a line-less.
fn parse_outcome(debug: Option<&str>) -> FinishOutcome {
    match debug {
        Some(s) if s.starts_with("Done") => FinishOutcome::Done,
        Some(s) if s.starts_with("Blocked") => FinishOutcome::Blocked,
        Some(s) if s.starts_with("Timeout") => FinishOutcome::Timeout,
        Some(s) if s.starts_with("Limit") => FinishOutcome::Limit,
        _ => FinishOutcome::Stuck,
    }
}

/// Parse the `queue built` `order` field (`#30 -> #31 -> #32`) into the issue
/// numbers in queue order. Tolerant of spacing and a missing/empty field.
fn parse_order(order: Option<&str>) -> Vec<u64> {
    let Some(s) = order else {
        return Vec::new();
    };
    s.split("->")
        .filter_map(|tok| {
            tok.trim()
                .trim_start_matches('#')
                .trim()
                .parse::<u64>()
                .ok()
        })
        .collect()
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

/// Render a [`UiAction`] to a single line, or `None` for [`UiAction::Ignore`].
/// The local timestamp and the outcome glyph are always present on a surfaced
/// line; colour is applied only when `opts.color` is set.
fn render_line(
    action: &UiAction,
    ts: &DateTime<Local>,
    duration: Option<Duration>,
    opts: RenderOpts,
) -> Option<String> {
    let ts_str = ts.format("%Y-%m-%d %H:%M:%S").to_string();
    let dur = duration
        .map(|d| format!(" ({})", fmt_duration(d)))
        .unwrap_or_default();

    let (glyph, style, body) = match action {
        UiAction::QueueBuilt { count, .. } => (
            pick("📋", "[queue]", opts.emoji),
            Style::new().cyan(),
            format!("queue built: {count} issue(s)"),
        ),
        // Live-region only: the active line carries the execution phase; no
        // permanent scroll-up line is drawn for it.
        UiAction::Executing { .. } => return None,
        UiAction::IssueStarted { number, title } => (
            pick("🧠", "[plan]", opts.emoji),
            Style::new().cyan(),
            format!("#{number} {title} — planning"),
        ),
        UiAction::PlanWritten { number, open_steps } => (
            pick("📝", "[plan]", opts.emoji),
            Style::new().cyan(),
            format!("#{number} plan written ({open_steps} step(s))"),
        ),
        UiAction::Finished { number, outcome } => {
            let (emoji, ascii, style) = outcome.glyph();
            (
                pick(emoji, ascii, opts.emoji),
                style,
                format!("#{number} {}{dur}", outcome.label()),
            )
        }
        UiAction::Skipped { number, kind } => (
            pick("⏭️", "[skip]", opts.emoji),
            Style::new().dim(),
            format!("#{number} {}{dur}", kind.label()),
        ),
        UiAction::Warn { message } => (
            pick("⚠️", "[warn]", opts.emoji),
            Style::new().yellow(),
            message.clone(),
        ),
        UiAction::Error { message } => (
            pick("💥", "[error]", opts.emoji),
            Style::new().red(),
            message.clone(),
        ),
        UiAction::Ignore => return None,
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

/// Render a [`UiAction`] to a plain, ANSI-free line (local timestamp + outcome
/// glyph + body). The non-TTY / `NO_COLOR` clean-line path; also the public seam
/// the unit tests assert against.
pub fn render_plain_line(
    action: &UiAction,
    ts: &DateTime<Local>,
    duration: Option<Duration>,
) -> Option<String> {
    render_line(
        action,
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

    /// Apply one classified action: drive the live region + active-issue tracking,
    /// then emit the permanent line (if any).
    fn apply(&self, action: UiAction) {
        let ts = Local::now();
        // Recover from poison rather than panic: this runs inside `on_event`, so a
        // panic here would corrupt the run on a tracing call.
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());

        let duration = self.drive(&mut s, &action);

        // Styled lines on a colour TTY (routed through `MultiProgress` so they
        // never tear the live region); one clean, ANSI-free line per event
        // otherwise (the non-TTY / `NO_COLOR` path, ADR-0006 D3).
        let line = if self.opts.color {
            render_line(&action, &ts, duration, self.opts)
        } else {
            render_plain_line(&action, &ts, duration)
        };
        if let Some(line) = line {
            if self.opts.color {
                let _ = self.multi.println(line);
            } else {
                eprintln!("{line}");
            }
        }
    }

    /// Update the live region for one action and return a finishing duration when
    /// the action closes the active issue. The live-region (`indicatif`) calls are
    /// guarded behind `self.opts.color`, so `--verbose`/non-TTY draw nothing.
    fn drive(&self, s: &mut LiveState, action: &UiAction) -> Option<Duration> {
        match action {
            UiAction::QueueBuilt { count, order } => {
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
            UiAction::IssueStarted { number, title } => {
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
            UiAction::Executing {
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
            UiAction::Finished { number, .. } | UiAction::Skipped { number, .. } => {
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
        let action = classify_event(event.metadata().target(), &fields.message, &fields);
        self.apply(action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fields(level: Level, message: &str) -> EventFields {
        EventFields {
            level,
            message: message.to_string(),
            ..EventFields::default()
        }
    }

    #[test]
    fn classify_queue_built() {
        let f = EventFields {
            count: Some(3),
            order: Some("#30 -> #31 -> #32".to_string()),
            ..fields(Level::INFO, "queue built")
        };
        assert_eq!(
            classify_event("ralphy", "queue built", &f),
            UiAction::QueueBuilt {
                count: 3,
                order: vec![30, 31, 32],
            }
        );
    }

    #[test]
    fn classify_issue_started() {
        let f = EventFields {
            number: Some(30),
            title: Some("Console UI".to_string()),
            ..fields(Level::INFO, "issue started")
        };
        assert_eq!(
            classify_event("ralphy_core::runner", "issue started", &f),
            UiAction::IssueStarted {
                number: 30,
                title: "Console UI".to_string(),
            }
        );
    }

    #[test]
    fn classify_plan_written() {
        let f = EventFields {
            number: Some(30),
            open_steps: Some(7),
            ..fields(Level::INFO, "plan written")
        };
        assert_eq!(
            classify_event("ralphy_core::runner", "plan written", &f),
            UiAction::PlanWritten {
                number: 30,
                open_steps: 7,
            }
        );
    }

    #[test]
    fn classify_green_closed_is_done() {
        let f = EventFields {
            number: Some(30),
            ..fields(Level::INFO, "green — issue closed")
        };
        assert_eq!(
            classify_event("ralphy_core::runner", "green — issue closed", &f),
            UiAction::Finished {
                number: 30,
                outcome: FinishOutcome::Done,
            }
        );
    }

    #[test]
    fn classify_non_green_maps_outcome() {
        for (debug, expected) in [
            ("Timeout", FinishOutcome::Timeout),
            ("Stuck", FinishOutcome::Stuck),
            ("Blocked(\"reason\")", FinishOutcome::Blocked),
            ("Limit(Some(\"15:00\"))", FinishOutcome::Limit),
        ] {
            let f = EventFields {
                number: Some(30),
                outcome: Some(debug.to_string()),
                ..fields(Level::INFO, "non-green — stopping run")
            };
            assert_eq!(
                classify_event("ralphy_core::runner", "non-green — stopping run", &f),
                UiAction::Finished {
                    number: 30,
                    outcome: expected,
                }
            );
        }
    }

    #[test]
    fn classify_blocked_skip() {
        let f = EventFields {
            number: Some(30),
            ..fields(Level::INFO, "blocked by open issue(s) — skipping")
        };
        assert_eq!(
            classify_event(
                "ralphy_core::runner",
                "blocked by open issue(s) — skipping",
                &f
            ),
            UiAction::Skipped {
                number: 30,
                kind: SkipKind::BlockedBy,
            }
        );
    }

    #[test]
    fn classify_stop_before_skip() {
        let msg = "stop-before label — halting run before this issue";
        let f = EventFields {
            number: Some(30),
            ..fields(Level::INFO, msg)
        };
        assert_eq!(
            classify_event("ralphy_core::runner", msg, &f),
            UiAction::Skipped {
                number: 30,
                kind: SkipKind::StopBefore,
            }
        );
    }

    #[test]
    fn classify_warn_and_error_by_level() {
        let w = fields(Level::WARN, "could not return to 'main'");
        assert_eq!(
            classify_event("ralphy_core::runner", &w.message.clone(), &w),
            UiAction::Warn {
                message: "could not return to 'main'".to_string(),
            }
        );
        let e = fields(Level::ERROR, "boom");
        assert_eq!(
            classify_event("ralphy", &e.message.clone(), &e),
            UiAction::Error {
                message: "boom".to_string(),
            }
        );
    }

    #[test]
    fn classify_unknown_info_is_ignored() {
        let f = fields(Level::INFO, "run branch created");
        assert_eq!(
            classify_event("ralphy_core::runner", "run branch created", &f),
            UiAction::Ignore
        );
    }

    #[test]
    fn render_plain_finished_carries_timestamp_glyph_and_no_ansi() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
            .single()
            .unwrap();
        let action = UiAction::Finished {
            number: 30,
            outcome: FinishOutcome::Done,
        };
        let line = render_plain_line(&action, &ts, Some(Duration::from_secs(133))).expect("a line");

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
    fn render_plain_ignore_is_none() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
            .single()
            .unwrap();
        assert_eq!(render_plain_line(&UiAction::Ignore, &ts, None), None);
    }

    #[test]
    fn fmt_duration_formats_minutes_and_seconds() {
        assert_eq!(fmt_duration(Duration::from_secs(13)), "13s");
        assert_eq!(fmt_duration(Duration::from_secs(133)), "2m13s");
        assert_eq!(fmt_duration(Duration::from_secs(120)), "2m00s");
    }

    #[test]
    fn classify_both_execution_events_map_to_executing() {
        for msg in [
            "executing with interactive claude over the PTY",
            "executing with headless claude -p loop",
        ] {
            // The adapter event carries no `number`; it carries model + budget_min.
            let f = EventFields {
                model: Some("sonnet".to_string()),
                budget_min: Some(45),
                ..fields(Level::INFO, msg)
            };
            assert_eq!(
                classify_event("ralphy_agent_claude", msg, &f),
                UiAction::Executing {
                    number: 0,
                    model: "sonnet".to_string(),
                    budget_min: 45,
                }
            );
        }
    }

    #[test]
    fn parse_order_round_trips_and_tolerates_edges() {
        assert_eq!(parse_order(Some("#30 -> #31 -> #32")), vec![30, 31, 32]);
        assert_eq!(parse_order(Some("#7")), vec![7]);
        assert_eq!(parse_order(None), Vec::<u64>::new());
        assert_eq!(parse_order(Some("")), Vec::<u64>::new());
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
