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

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use console::Style;
use indicatif::MultiProgress;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

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
    },
    IssueStarted {
        number: u64,
        title: String,
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
        }
    }
}

impl Visit for EventFields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "number" => self.number = Some(value),
            "open_steps" => self.open_steps = Some(value),
            "count" => self.count = Some(value),
            _ => {}
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "title" => self.title = Some(value.to_string()),
            "outcome" => self.outcome = Some(value.to_string()),
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
        },
        "issue started" => UiAction::IssueStarted {
            number,
            title: fields.title.clone().unwrap_or_default(),
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

/// How a line is rendered: whether ANSI colour and emoji are available. The
/// non-TTY / `NO_COLOR` path sets both `false`, guaranteeing no ANSI ever reaches
/// a redirected file (ADR-0006 D3).
#[derive(Debug, Clone, Copy)]
struct RenderOpts {
    color: bool,
    emoji: bool,
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
        UiAction::QueueBuilt { count } => (
            pick("📋", "[queue]", opts.emoji),
            Style::new().cyan(),
            format!("queue built: {count} issue(s)"),
        ),
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

/// The console presenter: a `tracing` Layer that renders the run's lifecycle. It
/// holds the active issue (number + monotonic start) so a finishing line can show
/// the issue's wall-clock duration, and a `MultiProgress` writer so on-screen
/// lines never corrupt one another.
pub struct Presenter {
    multi: MultiProgress,
    active: Mutex<Option<(u64, Instant)>>,
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
            active: Mutex::new(None),
            opts: RenderOpts {
                color: styled,
                emoji: styled,
            },
        }
    }

    /// Apply one classified action: track the active issue, compute a finishing
    /// duration, and emit the rendered line (if any).
    fn apply(&self, action: UiAction) {
        let ts = Local::now();

        if let UiAction::IssueStarted { number, .. } = &action {
            // Recover from poison rather than panic: this runs inside `on_event`,
            // so a panic here would corrupt the run on a tracing call.
            *self.active.lock().unwrap_or_else(|e| e.into_inner()) =
                Some((*number, Instant::now()));
        }

        // A finishing or skipping line closes out the active issue and carries its
        // duration when the numbers match.
        let duration = match &action {
            UiAction::Finished { number, .. } | UiAction::Skipped { number, .. } => {
                let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
                let d = active
                    .filter(|(n, _)| n == number)
                    .map(|(_, start)| start.elapsed());
                *active = None;
                d
            }
            _ => None,
        };

        // Styled lines on a colour TTY; one clean, ANSI-free line per event
        // otherwise (the non-TTY / `NO_COLOR` path, ADR-0006 D3).
        let line = if self.opts.color {
            render_line(&action, &ts, duration, self.opts)
        } else {
            render_plain_line(&action, &ts, duration)
        };
        if let Some(line) = line {
            self.emit(&line);
        }
    }

    /// Write one permanent line. On a TTY it goes through `MultiProgress` so it
    /// never tears a live region; otherwise straight to stderr.
    fn emit(&self, line: &str) {
        if self.opts.color {
            let _ = self.multi.println(line);
        } else {
            eprintln!("{line}");
        }
    }
}

impl Default for Presenter {
    fn default() -> Self {
        Self::new()
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
            ..fields(Level::INFO, "queue built")
        };
        assert_eq!(
            classify_event("ralphy", "queue built", &f),
            UiAction::QueueBuilt { count: 3 }
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
}
