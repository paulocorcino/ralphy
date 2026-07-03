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

use crate::pricing::PriceTable;
use crate::runstate::{event_to_runevent, EventFields, RunEvent, SkipKind};
// Re-exported because it appears in `PanelData`'s public fields (constructed in `main`).
pub use crate::runstate::UsageLite;

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
/// A human-return skip (ADR-0016) names the parking label when known
/// (`skipped (needs-info)`), falling back to the bare kind otherwise.
fn skip_label(kind: SkipKind, label: Option<&str>) -> String {
    match kind {
        SkipKind::BlockedBy => "skipped (blocked)".to_string(),
        SkipKind::StopBefore => "skipped (stop-before)".to_string(),
        SkipKind::HumanReturn => match label {
            Some(l) => format!("skipped ({l})"),
            None => "skipped (human-return)".to_string(),
        },
        SkipKind::VerifyFailed => "skipped (verify failed)".to_string(),
    }
}

/// A human-gate skip label, naming the blocker(s) the operator must clear: e.g.
/// `waiting on human at #30` (ADR-0014). Falls back to the bare phrase when the
/// blocker list is empty (a label fetch the runner could not resolve).
fn human_blocked_label(on: &[u64]) -> String {
    if on.is_empty() {
        return "waiting on human".to_string();
    }
    let at = on
        .iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("waiting on human at {at}")
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

fn sleep_label(reset: &str, opts: RenderOpts) -> String {
    format!(
        "{} usage limit — sleeping until {reset}",
        pick("🌙", "[limit]", opts.emoji)
    )
}

/// Render the live active-issue line: phase icon · `#n` title · model · `elapsed`
/// (or `elapsed / budget`). Pure over its inputs; the emoji/ASCII and colour
/// choice come from `opts`. The non-colour path emits no ANSI byte.
#[allow(clippy::too_many_arguments)]
fn render_active_line(
    phase: Phase,
    number: u64,
    title: &str,
    model: Option<&str>,
    effort: Option<&str>,
    elapsed: Duration,
    budget_min: Option<u64>,
    opts: RenderOpts,
) -> String {
    let icon = match phase {
        Phase::Planning => pick("🧠", "[plan]", opts.emoji),
        Phase::Executing => pick("⚙️", "[exec]", opts.emoji),
    };
    let mut parts: Vec<String> = vec![format!("{icon} #{number} {title}")];
    if let Some(seg) = model_effort_seg(model, effort) {
        parts.push(if opts.color {
            Style::new().cyan().apply_to(seg).to_string()
        } else {
            seg
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
    /// Issues stalled on a human gate in their path (ADR-0014) — surfaced in the
    /// counts line only when non-zero, so ordinary runs stay unchanged.
    pub hitl: u64,
    pub commits: usize,
    pub stop: Option<PanelStop>,
    pub branch_mode: PanelBranchMode,
    pub dry_run: bool,
    /// The run's local `ralphy/pre-run-<stamp>` undo tag, when one exists (the
    /// runner deletes it on a zero-commit run). Drives the `↩ undo:` line.
    pub undo_tag: Option<String>,
    /// This run's token breakdown across all phases, for the compact footer meter
    /// (ADR-0008 D11). `model` is unused at the footer — USD is supplied below.
    pub run_breakdown: UsageLite,
    /// The project's cumulative token breakdown, read from the ledger after the run.
    pub project_breakdown: UsageLite,
    /// The project slug (`owner/repo` or a path-hash) shown in the footer.
    pub project_id: String,
    /// Read-time USD for this run (ADR-0008 D8), priced per model. `None` when
    /// nothing in the set could be priced — rendered `~$?`, never `~$0.00`.
    pub run_usd: Option<f64>,
    /// Read-time USD for the project's cumulative ledger, priced per model.
    pub project_usd: Option<f64>,
    /// Whether any model in *this run* was unpriced — the run figure then carries a
    /// `+?` suffix. Tracked separately from the project so a fully-priced run is not
    /// flagged `+?` merely because the cumulative ledger holds an unpriced model.
    pub run_usd_partial: bool,
    /// Whether any model in the *cumulative project* ledger was unpriced — the
    /// project figure then carries the `+?` suffix, independent of the run.
    pub project_usd_partial: bool,
}

/// Render a [`RunEvent`] to a single line, or `None` for live-region-only events.
/// The local timestamp and the outcome glyph are always present on a surfaced
/// line; colour is applied only when `opts.color` is set.
fn render_line(
    event: &RunEvent,
    ts: &DateTime<Local>,
    extra: &LineExtra,
    opts: RenderOpts,
) -> Option<String> {
    if opts.color && matches!(event, RunEvent::SleepStarted { .. }) {
        return None;
    }

    let ts_str = ts.format("%Y-%m-%d %H:%M:%S").to_string();
    // Generic finished-line duration (` (2m13s)`) for the non-issue outcome lines;
    // the issue lines (`plan written` / `done`) compose their own tail via `issue_tail`.
    let dur = extra
        .duration
        .map(|d| format!(" ({})", fmt_duration(d)))
        .unwrap_or_default();

    let (glyph, style, body) = match event {
        RunEvent::QueueBuilt { count, .. } => (
            pick("📋", "[queue]", opts.emoji),
            Style::new().cyan(),
            format!("queue built: {count} issue(s)"),
        ),
        // Live-region only: the active line carries the planning/execution phase
        // and its model/effort; no permanent scroll-up line is drawn for them.
        RunEvent::Executing { .. } | RunEvent::Planning { .. } => return None,
        // The ADR-0019 run-boundary events are for the CloudEvents sink only; the
        // console already draws its own header/panel, so no scroll-up line here.
        RunEvent::RunStarted { .. } | RunEvent::RunFinished { .. } => return None,
        RunEvent::IssueStarted { number, title } => (
            pick("🧠", "[plan]", opts.emoji),
            Style::new().cyan(),
            format!("#{number} {title} — planning"),
        ),
        RunEvent::PlanWritten {
            number, open_steps, ..
        } => (
            pick("📝", "[plan]", opts.emoji),
            Style::new().cyan(),
            issue_tail(
                *number,
                &format!("plan written ({open_steps} step(s))"),
                extra,
                opts,
            ),
        ),
        RunEvent::IssueClosed { number, .. } => {
            let outcome = FinishOutcome::Done;
            let (emoji, ascii, style) = outcome.glyph();
            (
                pick(emoji, ascii, opts.emoji),
                style,
                issue_tail(*number, outcome.label(), extra, opts),
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
        RunEvent::Skipped {
            number,
            kind,
            label,
        } => (
            pick("⏭️", "[skip]", opts.emoji),
            Style::new().dim(),
            format!("#{number} {}{dur}", skip_label(*kind, label.as_deref())),
        ),
        // A human gate gets its own glyph (🙋) and a non-dim style: it asks for a
        // person — and names which issue (`at #30`) — unlike an ordinary
        // dependency skip the queue clears on its own (ADR-0014).
        RunEvent::HumanBlocked { number, on } => (
            pick("🙋", "[hitl]", opts.emoji),
            Style::new().yellow(),
            format!("#{number} {}{dur}", human_blocked_label(on)),
        ),
        RunEvent::NeedsSplit { number } => (
            pick("🧩", "[split]", opts.emoji),
            Style::new().yellow(),
            format!("#{number} bundle — needs split (run /to-issues, close the bundle)"),
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
        RunEvent::KnowledgeConsolidating { notes } => (
            pick("📚", "[know]", opts.emoji),
            Style::new().cyan(),
            format!("consolidating {notes} knowledge note(s) into KNOWLEDGE.md"),
        ),
        RunEvent::KnowledgeConsolidated { archived } => (
            pick("📚", "[know]", opts.emoji),
            Style::new().green(),
            format!("knowledge consolidated — {archived} note(s) archived into knowledge/raw/"),
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
#[cfg(test)]
fn render_plain_line(
    event: &RunEvent,
    ts: &DateTime<Local>,
    duration: Option<Duration>,
) -> Option<String> {
    render_line(
        event,
        ts,
        &LineExtra {
            duration,
            ..Default::default()
        },
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

    // "Waiting on human" bucket (ADR-0014) — appended only when something is
    // stalled on a human gate, so ordinary runs keep their three-part line.
    if data.hitl > 0 {
        let hitl_icon = pick("🙋", "[hitl]", opts.emoji);
        let hitl_raw = format!("{hitl_icon} {} waiting on human", data.hitl);
        lines.push(if opts.color {
            Style::new().yellow().apply_to(&hitl_raw).to_string()
        } else {
            hitl_raw
        });
    }

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

    // Undo line: only when the run left commits behind and the pre-run tag
    // exists (the runner deletes it on a zero-commit run). The command is
    // mode-aware — `Current` rewinds the live branch to the marker; `New`
    // simply drops the run branch (checking out `orig` first when a stop left
    // the repo parked on it).
    if data.commits > 0 {
        if let Some(tag) = &data.undo_tag {
            let undo_icon = pick("↩️", "[undo]", opts.emoji);
            let cmd = match data.branch_mode {
                PanelBranchMode::Current => format!("git reset --hard {tag}"),
                PanelBranchMode::New if stopped => format!(
                    "git checkout {} && git branch -D {}",
                    data.orig_branch, data.branch
                ),
                PanelBranchMode::New => format!("git branch -D {}", data.branch),
            };
            let undo_raw = format!("{undo_icon}  undo (pre-run tag '{tag}'): {cmd}");
            lines.push(if opts.color {
                Style::new().dim().apply_to(&undo_raw).to_string()
            } else {
                undo_raw
            });
        }
    }

    // Next-step line: New mode only, absent when dry-run + zero commits
    if data.branch_mode == PanelBranchMode::New && !(data.dry_run && data.commits == 0) {
        let next_raw = format!("➜  git merge {}", data.branch);
        lines.push(if opts.color {
            Style::new().cyan().apply_to(&next_raw).to_string()
        } else {
            next_raw
        });
    }

    // Token-usage footer (ADR-0008 D11): the run total and the project's
    // accumulated balance, each in tokens plus a read-time USD estimate (D8). USD
    // is a read-time projection, never stored; an unpriced model shows `~$?`
    // (never `~$0.00`) or flags the priced portion with `+?`.
    let footer_raw = format!(
        "run: {} · project: {} {}",
        fmt_breakdown(
            &data.run_breakdown,
            data.run_usd,
            data.run_usd_partial,
            opts.emoji
        ),
        data.project_id,
        fmt_breakdown(
            &data.project_breakdown,
            data.project_usd,
            data.project_usd_partial,
            opts.emoji
        ),
    );
    lines.push(if opts.color {
        Style::new().dim().apply_to(&footer_raw).to_string()
    } else {
        footer_raw
    });

    lines
}

/// Format a footer breakdown meter — `↑X ⚡X ❄X ↓X · $Y` — reusing the inline meter
/// layout with an externally-supplied read-time USD (the footer prices the per-model
/// split in `main`, not a single [`UsageLite`]).
fn fmt_breakdown(u: &UsageLite, usd: Option<f64>, partial: bool, emoji: bool) -> String {
    fmt_meter(
        &Meter {
            usage: u.clone(),
            usd,
            partial,
        },
        emoji,
    )
}

/// A priced token meter for one scroll-up line: the combined breakdown to show
/// (`↑ ⚡ ❄ ↓`) plus the read-time USD (D8). `usd` is `None` when nothing in the
/// meter could be priced (rendered `$?`, never `$0`); `partial` flags a model that
/// was unpriced so the figure can carry a `+?` residue.
struct Meter {
    usage: UsageLite,
    usd: Option<f64>,
    partial: bool,
}

/// Per-line render context the presenter computes in `drive` (it owns the clock,
/// the active issue's display model/effort, and the price table) and hands to
/// `render_line`. All fields are absent for events that carry no meter/duration.
#[derive(Default)]
struct LineExtra {
    duration: Option<Duration>,
    model: Option<String>,
    effort: Option<String>,
    meter: Option<Meter>,
}

/// Price one phase's [`UsageLite`] at read time, or `None` when its model is absent
/// or unpriced. Bridges to the core `Usage` the [`PriceTable`] prices on.
fn price_lite(pt: &PriceTable, u: &UsageLite) -> Option<f64> {
    let model = u.model.as_deref().filter(|m| !m.is_empty())?;
    let usage = ralphy_core::Usage {
        input: u.input,
        output: u.output,
        cache_read: u.cache_read,
        cache_creation: u.cache_creation,
        model: u.model.clone(),
    };
    pt.cost_usd(model, &usage)
}

/// Build a [`Meter`] for an issue line from its planning usage (stashed, may be
/// absent on the `plan written` line) and a final phase's usage. The display
/// breakdown sums both phases; the USD prices each phase's model separately (plan
/// and execute often differ) and sums the priced portion, mirroring
/// `cost_usd_by_model`'s `$?`/`+?` semantics (ADR-0008 D8).
fn meter_for(pt: &PriceTable, plan: Option<&UsageLite>, last: &UsageLite) -> Meter {
    let mut combined = last.clone();
    combined.model = None; // the sum spans models; the label's model comes from the active issue
    if let Some(p) = plan {
        combined.input += p.input;
        combined.cache_read += p.cache_read;
        combined.cache_creation += p.cache_creation;
        combined.output += p.output;
    }
    let mut usd = 0.0;
    let mut any_priced = false;
    let mut any_unpriced = false;
    for u in plan.into_iter().chain(std::iter::once(last)) {
        if u.total() == 0 {
            continue;
        }
        match price_lite(pt, u) {
            Some(c) => {
                usd += c;
                any_priced = true;
            }
            None => any_unpriced = true,
        }
    }
    Meter {
        usage: combined,
        usd: any_priced.then_some(usd),
        partial: any_unpriced,
    }
}

/// The compact emoji token meter: `↑12.4k ⚡184k ❄8.1k ↓3.2k · $1.84`. `↑` input,
/// `⚡` cache-read (hot reuse), `❄` cache-write (cold store), `↓` output. The ASCII
/// path drops the emoji glyphs for `in/cr/cw/out` labels.
fn fmt_meter(m: &Meter, emoji: bool) -> String {
    let u = &m.usage;
    let (i, cr, cw, o) = if emoji {
        ("↑", "⚡", "❄", "↓")
    } else {
        ("in ", "cr ", "cw ", "out ")
    };
    format!(
        "{i}{} {cr}{} {cw}{} {o}{} · {}",
        fmt_tokens(u.input),
        fmt_tokens(u.cache_read),
        fmt_tokens(u.cache_creation),
        fmt_tokens(u.output),
        fmt_usd_compact(m.usd, m.partial),
    )
}

/// Compact read-time USD for an inline meter: `$1.84`, `$1.84+?` when some model was
/// unpriced, or a bare `$?` when nothing could be priced — never `$0.00`, which would
/// hide spend (ADR-0008 D8).
fn fmt_usd_compact(usd: Option<f64>, partial: bool) -> String {
    match usd {
        None => "$?".to_string(),
        Some(v) => format!("${v:.2}{}", if partial { "+?" } else { "" }),
    }
}

/// The `model / effort` label segment for an issue line, from the active issue's
/// display values. `None` when no model is known; effort alone is never shown.
fn model_effort_seg(model: Option<&str>, effort: Option<&str>) -> Option<String> {
    match (model, effort) {
        (Some(m), Some(e)) => Some(format!("{m} / {e}")),
        (Some(m), None) => Some(m.to_string()),
        _ => None,
    }
}

/// Build an issue scroll-line body — `#N <label> · model / effort · (dur) · meter`
/// — appending only the segments that are present, joined by ` · `. Shared by the
/// `plan written` and `done` lines so their layout stays identical.
fn issue_tail(number: u64, label: &str, extra: &LineExtra, opts: RenderOpts) -> String {
    let mut tail: Vec<String> = Vec::new();
    if let Some(seg) = model_effort_seg(extra.model.as_deref(), extra.effort.as_deref()) {
        tail.push(seg);
    }
    if let Some(d) = extra.duration {
        tail.push(format!("({})", fmt_duration(d)));
    }
    if let Some(m) = extra.meter.as_ref().filter(|m| m.usage.total() > 0) {
        tail.push(fmt_meter(m, opts.emoji));
    }
    if tail.is_empty() {
        format!("#{number} {label}")
    } else {
        format!("#{number} {label} · {}", tail.join(" · "))
    }
}

/// Format a token count compactly for the footer: `1.2M`, `8.4k`, or a bare
/// `912` under a thousand. One decimal place for the scaled forms.
fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Normalize a git remote URL to an `https` web URL for the header link: strip a
/// trailing `.git`, and rewrite the `git@host:owner/repo` / `ssh://git@host/owner/repo`
/// SSH forms to `https://host/owner/repo`. An already-`http(s)` URL is left as-is
/// (minus `.git`). Pure over its input.
pub fn normalize_remote_url(raw: &str) -> String {
    let s = raw.trim();
    let s = s.strip_suffix(".git").unwrap_or(s);
    if let Some(rest) = s.strip_prefix("ssh://git@") {
        return format!("https://{rest}");
    }
    if let Some(rest) = s.strip_prefix("git@") {
        // `host:owner/repo` → `host/owner/repo` (only the first colon is the sep).
        return format!("https://{}", rest.replacen(':', "/", 1));
    }
    s.to_string()
}

/// Render the start-up info line shown under the branding header: the project name,
/// the current branch, and the repo web URL, joined by ` · `. Each present segment
/// gets an emoji prefix only when `opts.emoji`; a missing branch or URL is simply
/// omitted. The non-colour path emits no ANSI byte.
pub fn render_info_line(
    project: &str,
    branch: Option<&str>,
    url: Option<&str>,
    opts: RenderOpts,
) -> String {
    let seg = |emoji: &str, value: &str| -> String {
        if opts.emoji {
            format!("{emoji} {value}")
        } else {
            value.to_string()
        }
    };
    let mut parts: Vec<String> = vec![seg("📦", project)];
    if let Some(b) = branch {
        parts.push(seg("🌿", b));
    }
    if let Some(u) = url {
        parts.push(seg("🔗", u));
    }
    let line = parts.join(" · ");
    if opts.color {
        Style::new().dim().apply_to(&line).to_string()
    } else {
        line
    }
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
        let event = RunEvent::IssueClosed {
            number: 30,
            tokens: 0,
            usage: UsageLite::default(),
        };
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
    fn render_done_line_shows_model_effort_duration_and_compact_meter() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
            .single()
            .unwrap();
        // The meter/model/effort/duration ride on `LineExtra` (the presenter computes
        // them in `drive`); the event itself only names the issue.
        let extra = LineExtra {
            duration: Some(Duration::from_secs(776)),
            model: Some("sonnet".into()),
            effort: Some("medium".into()),
            meter: Some(Meter {
                usage: UsageLite {
                    input: 41_200,
                    cache_read: 902_000,
                    cache_creation: 22_000,
                    output: 18_400,
                    model: None,
                },
                usd: Some(6.10),
                partial: false,
            }),
        };
        let line = render_line(
            &RunEvent::IssueClosed {
                number: 45,
                tokens: 0,
                usage: UsageLite::default(),
            },
            &ts,
            &extra,
            RenderOpts {
                color: false,
                emoji: true,
            },
        )
        .expect("a line");
        assert!(line.contains("#45 done"), "issue + outcome: {line}");
        assert!(line.contains("sonnet / medium"), "model / effort: {line}");
        assert!(line.contains("(12m56s)"), "issue duration: {line}");
        // Compact emoji breakdown + USD, scaled and joined by ` · `.
        assert!(line.contains("↑41.2k"), "input glyph + tokens: {line}");
        assert!(line.contains("⚡902.0k"), "cache-read: {line}");
        assert!(line.contains("❄22.0k"), "cache-write: {line}");
        assert!(line.contains("↓18.4k"), "output: {line}");
        assert!(line.contains("$6.10"), "read-time USD: {line}");
        assert!(!line.contains('\u{1b}'), "no ANSI byte: {line:?}");
    }

    #[test]
    fn render_done_line_omits_meter_when_zero() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
            .single()
            .unwrap();
        let event = RunEvent::IssueClosed {
            number: 9,
            tokens: 0,
            usage: UsageLite::default(),
        };
        let line = render_plain_line(&event, &ts, None).expect("a line");
        assert!(line.contains("#9 done"), "issue + outcome: {line}");
        assert!(!line.contains('↑'), "no meter when usage is zero: {line}");
        assert!(!line.contains('$'), "no cost when usage is zero: {line}");
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
                    effort: None,
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
                    effort: None,
                },
                &ts,
                None,
            ),
            None
        );
    }

    #[test]
    fn styled_sleep_started_is_live_region_only() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
            .single()
            .unwrap();
        let event = RunEvent::SleepStarted {
            reset: "08:10".to_string(),
            target_epoch: 1_000_000,
        };
        let styled = RenderOpts {
            color: true,
            emoji: true,
        };
        assert_eq!(
            render_line(&event, &ts, &LineExtra::default(), styled),
            None
        );

        let plain = RenderOpts {
            color: false,
            emoji: true,
        };
        assert!(render_line(&event, &ts, &LineExtra::default(), plain)
            .expect("plain sleep line")
            .contains("sleeping until 08:10"));
    }

    #[test]
    fn render_plain_knowledge_consolidation_carries_glyph_and_counts() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 14, 2, 16, 0)
            .single()
            .unwrap();
        let started = render_plain_line(&RunEvent::KnowledgeConsolidating { notes: 4 }, &ts, None)
            .expect("KnowledgeConsolidating renders a line");
        assert!(started.contains('📚'), "knowledge glyph: {started}");
        assert!(started.contains('4'), "note count: {started}");
        assert!(started.contains("KNOWLEDGE.md"), "target file: {started}");

        let done = render_plain_line(&RunEvent::KnowledgeConsolidated { archived: 4 }, &ts, None)
            .expect("KnowledgeConsolidated renders a line");
        assert!(done.contains('📚'), "knowledge glyph: {done}");
        assert!(
            done.contains("4 note(s) archived"),
            "archived count: {done}"
        );
        assert!(!done.contains('\u{1b}'), "no ANSI byte: {done:?}");
    }

    #[test]
    fn render_plain_needs_split_names_the_bundle_and_next_step() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
            .single()
            .unwrap();
        let line = render_plain_line(&RunEvent::NeedsSplit { number: 3 }, &ts, None)
            .expect("NeedsSplit renders a line");
        assert!(line.contains("#3 bundle — needs split"), "{line}");
        assert!(line.contains("/to-issues"), "{line}");
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
                label: None,
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
                label: None,
            },
            &ts,
            None,
        )
        .expect("StopBefore renders a line");
        assert!(
            stop_before.contains("skipped (stop-before)"),
            "{stop_before}"
        );

        let human_return = render_plain_line(
            &RunEvent::Skipped {
                number: 9,
                kind: SkipKind::HumanReturn,
                label: Some("needs-info".to_string()),
            },
            &ts,
            None,
        )
        .expect("HumanReturn renders a line");
        assert!(
            human_return.contains("skipped (needs-info)"),
            "{human_return}"
        );
    }

    #[test]
    fn render_plain_human_blocked_names_the_blocker() {
        let ts = Local
            .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
            .single()
            .unwrap();
        let hitl = render_plain_line(
            &RunEvent::HumanBlocked {
                number: 16,
                on: vec![30],
            },
            &ts,
            None,
        )
        .expect("HumanBlocked renders a line");
        // Names the issue the operator must clear, not a bare phrase.
        assert!(hitl.contains("#16 waiting on human at #30"), "{hitl}");
        // The human-gate glyph (🙋), not the generic skip glyph, and no "skipped"
        // wording — it asks for a person, not a queue retry.
        assert!(hitl.contains("🙋"), "{hitl}");
        assert!(!hitl.contains("skipped"), "{hitl}");

        // With no resolved blocker, it degrades to the bare phrase.
        let bare = render_plain_line(
            &RunEvent::HumanBlocked {
                number: 7,
                on: vec![],
            },
            &ts,
            None,
        )
        .expect("HumanBlocked renders a line");
        assert!(bare.contains("#7 waiting on human"), "{bare}");
        assert!(!bare.contains(" at "), "{bare}");
    }

    #[test]
    fn totals_panel_shows_waiting_on_human_only_when_nonzero() {
        let opts = RenderOpts {
            color: false,
            emoji: false,
        };
        // Zero → the counts line stays the three-part triad, no hitl line.
        let none = render_totals_panel(&panel_base(), opts);
        assert!(
            !none.iter().any(|l| l.contains("waiting on human")),
            "{none:?}"
        );

        // Non-zero → a dedicated waiting-on-human line appears.
        let data = PanelData {
            hitl: 2,
            ..panel_base()
        };
        let lines = render_totals_panel(&data, opts);
        assert!(
            lines.iter().any(|l| l.contains("2 waiting on human")),
            "{lines:?}"
        );
    }

    #[test]
    fn totals_panel_undo_line_is_mode_aware() {
        let opts = RenderOpts {
            color: false,
            emoji: false,
        };

        // New + clean run (repo back on orig): undo drops the run branch.
        let new_clean = render_totals_panel(&panel_base(), opts);
        assert!(
            new_clean.iter().any(|l| l.contains(
                "undo (pre-run tag 'ralphy/pre-run-20260610-120000'): \
                 git branch -D afk/run-20260610-120000"
            )),
            "{new_clean:?}"
        );

        // New + stopped (repo parked on the run branch): checkout orig first.
        let stopped = render_totals_panel(
            &PanelData {
                stop: Some(PanelStop::Deadline),
                ..panel_base()
            },
            opts,
        );
        assert!(
            stopped
                .iter()
                .any(|l| l.contains("git checkout main && git branch -D afk/run-20260610-120000")),
            "{stopped:?}"
        );

        // Current: undo rewinds the live branch to the marker.
        let current = render_totals_panel(
            &PanelData {
                branch_mode: PanelBranchMode::Current,
                branch: "main".to_string(),
                ..panel_base()
            },
            opts,
        );
        assert!(
            current
                .iter()
                .any(|l| l.contains("git reset --hard ralphy/pre-run-20260610-120000")),
            "{current:?}"
        );
    }

    #[test]
    fn totals_panel_undo_line_absent_without_tag_or_commits() {
        let opts = RenderOpts {
            color: false,
            emoji: false,
        };
        // No tag (creation failed or the runner dropped it) → no undo line.
        let no_tag = render_totals_panel(
            &PanelData {
                undo_tag: None,
                ..panel_base()
            },
            opts,
        );
        assert!(!no_tag.iter().any(|l| l.contains("undo")), "{no_tag:?}");

        // Zero commits → nothing to undo, even if a tag value leaked through.
        let no_commits = render_totals_panel(
            &PanelData {
                commits: 0,
                ..panel_base()
            },
            opts,
        );
        assert!(
            !no_commits.iter().any(|l| l.contains("undo")),
            "{no_commits:?}"
        );
    }

    #[test]
    fn normalize_remote_url_handles_ssh_https_and_dot_git() {
        // SCP-style SSH → https, `.git` stripped.
        assert_eq!(
            normalize_remote_url("git@github.com:paulocorcino/ocs-inventory-go-server.git"),
            "https://github.com/paulocorcino/ocs-inventory-go-server"
        );
        // ssh:// URL form.
        assert_eq!(
            normalize_remote_url("ssh://git@github.com/owner/repo.git"),
            "https://github.com/owner/repo"
        );
        // Already https, only `.git` removed.
        assert_eq!(
            normalize_remote_url("https://github.com/owner/repo.git"),
            "https://github.com/owner/repo"
        );
        // https without `.git` is left intact.
        assert_eq!(
            normalize_remote_url("https://github.com/owner/repo"),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn render_info_line_emoji_plain_and_omits_missing_segments() {
        let emoji = RenderOpts {
            color: false,
            emoji: true,
        };
        let full = render_info_line(
            "ocs-inventory",
            Some("main"),
            Some("https://github.com/owner/repo"),
            emoji,
        );
        assert_eq!(
            full,
            "📦 ocs-inventory · 🌿 main · 🔗 https://github.com/owner/repo"
        );

        // No URL (local-only repo): the 🔗 segment is omitted entirely.
        let no_url = render_info_line("proj", Some("dev"), None, emoji);
        assert_eq!(no_url, "📦 proj · 🌿 dev");

        // Plain path: no emoji, no ANSI byte.
        let plain = render_info_line(
            "proj",
            Some("dev"),
            Some("https://x/y"),
            RenderOpts {
                color: false,
                emoji: false,
            },
        );
        assert_eq!(plain, "proj · dev · https://x/y");
        assert!(!plain.contains('\u{1b}'), "no ANSI byte: {plain:?}");
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
        let mut q = QueueState::built(5, vec![10, 11, 12, 13, 14], None);
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
        let mut q = QueueState::built(3, vec![1, 2, 3], None);
        q.advance(1);
        q.advance(2);
        assert_eq!(q.bar_label(), "▰▰▱ 2/3 (pending #3)");
        q.finish();
        assert_eq!(q.bar_label(), "▰▰▰ 3/3");
    }

    #[test]
    fn queue_state_marks_the_stop_before_cut_in_the_pending_list() {
        // The bioledger order: the run works #21..#10, then halts before #15.
        let q = QueueState::built(
            13,
            vec![21, 20, 14, 7, 8, 9, 10, 15, 16, 17, 18, 19, 5],
            Some(15),
        );
        let emoji = RenderOpts {
            color: false,
            emoji: true,
        };
        assert_eq!(
            q.bar_label_opts(emoji),
            "▱▱▱▱▱▱▱▱▱▱▱▱▱ 0/13 \
             (pending #21 #20 #14 #7 #8 #9 #10 ⛔ stop-before #15 #16 #17 #18 #19 #5)"
        );
        // ASCII fallback for a no-emoji terminal: same cut, glyph-free marker.
        let ascii = RenderOpts {
            color: false,
            emoji: false,
        };
        assert!(
            q.bar_label_opts(ascii)
                .contains("#10 |stop-before #15| #16"),
            "ascii marker brackets the boundary issue: {}",
            q.bar_label_opts(ascii)
        );
        // No `stop_before` → no marker, unchanged rendering.
        let plain = QueueState::built(2, vec![1, 2], None);
        assert_eq!(plain.bar_label(), "▱▱ 0/2 (pending #1 #2)");
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
            Some("medium"),
            Duration::from_secs(12 * 60 + 43),
            Some(45),
            opts,
        );
        assert!(line.contains('⚙'), "executing phase icon: {line}");
        assert!(line.contains("#31"), "issue number: {line}");
        assert!(line.contains("Console UI"), "title: {line}");
        assert!(line.contains("sonnet / medium"), "model / effort: {line}");
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
            Some("high"),
            Duration::from_secs(63),
            Some(45),
            opts,
        );
        assert!(line.contains("[exec]"), "ascii phase fallback: {line}");
        assert!(!line.contains('\u{1b}'), "no ANSI byte: {line:?}");
    }

    #[test]
    fn bar_label_no_colour_emits_no_ansi() {
        let mut q = QueueState::built(6, vec![1, 2, 3, 4, 5, 6], None);
        q.advance(1);
        q.advance(2);
        q.advance(3);
        let label = q.bar_label();
        assert_eq!(label, "▰▰▰▱▱▱ 3/6 (pending #4 #5 #6)");
        assert!(!label.contains('\u{1b}'), "no ANSI byte: {label:?}");
    }

    #[test]
    fn sleep_label_replaces_queue_context_with_limit_message() {
        let opts = RenderOpts {
            color: false,
            emoji: true,
        };
        let label = sleep_label("08:10", opts);
        assert_eq!(label, "🌙 usage limit — sleeping until 08:10");
        assert!(
            !label.contains("pending"),
            "sleep hides pending list: {label}"
        );
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
            hitl: 0,
            commits: 5,
            stop: None,
            branch_mode: PanelBranchMode::New,
            dry_run: false,
            undo_tag: Some("ralphy/pre-run-20260610-120000".to_string()),
            run_breakdown: UsageLite {
                input: 8_400_000,
                ..Default::default()
            },
            project_breakdown: UsageLite {
                input: 142_000_000,
                ..Default::default()
            },
            project_id: "owner/repo".to_string(),
            run_usd: Some(2.10),
            project_usd: Some(35.6),
            run_usd_partial: false,
            project_usd_partial: false,
        }
    }

    #[test]
    fn fmt_tokens_scales_millions_thousands_and_bare() {
        assert_eq!(fmt_tokens(1_200_000), "1.2M");
        assert_eq!(fmt_tokens(8_400), "8.4k");
        assert_eq!(fmt_tokens(912), "912");
        assert_eq!(fmt_tokens(0), "0");
    }

    #[test]
    fn render_totals_panel_footer_shows_run_and_project_tokens() {
        let opts = RenderOpts {
            color: false,
            emoji: true,
        };
        let lines = render_totals_panel(&panel_base(), opts);
        let footer = lines
            .iter()
            .find(|l| l.contains("run:") && l.contains("project:"))
            .expect("a token footer line");
        // Carries the formatted run breakdown, the project id, and the project total.
        assert!(footer.contains("↑8.4M"), "run input total: {footer}");
        assert!(footer.contains("owner/repo"), "project id: {footer}");
        assert!(footer.contains("↑142.0M"), "project input total: {footer}");
        // Read-time USD estimates (ADR-0008 D8), compact `$` form.
        assert!(footer.contains("$2.10"), "run usd: {footer}");
        assert!(footer.contains("$35.60"), "project usd: {footer}");
        assert!(!footer.contains('\u{1b}'), "no ANSI byte: {footer:?}");
    }

    #[test]
    fn render_totals_panel_footer_shows_unknown_usd_never_zero() {
        let opts = RenderOpts {
            color: false,
            emoji: true,
        };
        // A fully-unpriced run shows `~$?`, never `~$0.00`.
        let data = PanelData {
            run_usd: None,
            project_usd: None,
            run_usd_partial: true,
            project_usd_partial: true,
            ..panel_base()
        };
        let lines = render_totals_panel(&data, opts);
        let footer = lines
            .iter()
            .find(|l| l.contains("run:") && l.contains("project:"))
            .expect("a token footer line");
        assert!(footer.contains("$?"), "unknown usd shows $?: {footer}");
        assert!(
            !footer.contains("$0.00"),
            "never reports $0 for unknown spend: {footer}"
        );
    }

    #[test]
    fn render_totals_panel_run_and_project_partial_are_independent() {
        let opts = RenderOpts {
            color: false,
            emoji: true,
        };
        // A fully-priced run (no unpriced model) alongside a project whose
        // cumulative ledger DOES hold an unpriced model: the run figure must stay
        // clean while only the project carries `+?`. Guards the regression where a
        // single shared flag leaked the project's residue onto the run total.
        let data = PanelData {
            run_usd: Some(1.81),
            project_usd: Some(15.98),
            run_usd_partial: false,
            project_usd_partial: true,
            ..panel_base()
        };
        let lines = render_totals_panel(&data, opts);
        let footer = lines
            .iter()
            .find(|l| l.contains("run:") && l.contains("project:"))
            .expect("a token footer line");
        let (run_part, project_part) = footer.split_once("project:").expect("two segments");
        assert!(
            run_part.contains("$1.81") && !run_part.contains("+?"),
            "the fully-priced run must NOT carry +?: {run_part}"
        );
        assert!(
            project_part.contains("$15.98+?"),
            "the project's unpriced residue is flagged with +?: {project_part}"
        );
    }

    #[test]
    fn fmt_usd_compact_partial_suffix_and_unknown() {
        assert_eq!(fmt_usd_compact(Some(2.10), false), "$2.10");
        assert_eq!(fmt_usd_compact(Some(2.10), true), "$2.10+?");
        assert_eq!(fmt_usd_compact(None, false), "$?");
        assert_eq!(fmt_usd_compact(None, true), "$?");
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
