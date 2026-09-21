//! One line per [`RunEvent`] and the live active line.

use std::time::Duration;

use chrono::{DateTime, Local};
use console::Style;
use tracing::Level;

use super::meter::fmt_meter;
use super::{fmt_clock, fmt_duration, pick, LineExtra, RenderOpts};
use crate::runstate::{RunEvent, SkipKind};
use crate::ui::{fit, FinishOutcome, Phase};

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
/// A blocked-by skip names the still-open blocker(s) when known (`skipped (blocked
/// by #139)`), so the operator knows where to act — falling back to the bare
/// `skipped (blocked)` when the list is empty. A human-return skip (ADR-0016) names
/// the parking label when known (`skipped (needs-info)`), falling back to the bare
/// kind otherwise.
fn skip_label(kind: SkipKind, label: Option<&str>, blockers: &[u64]) -> String {
    match kind {
        SkipKind::BlockedBy if !blockers.is_empty() => {
            let by = blockers
                .iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("skipped (blocked by {by})")
        }
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

pub(crate) fn sleep_label(reset: &str, opts: RenderOpts) -> String {
    format!(
        "{} usage limit — sleeping until {reset}",
        pick("🌙", "[limit]", opts.emoji)
    )
}

/// Render the live active-issue line: phase icon · `#n` title · model · `elapsed`
/// (or `elapsed / budget`). Pure over its inputs; the emoji/ASCII and colour
/// choice come from `opts`. The non-colour path emits no ANSI byte.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_active_line(
    phase: Phase,
    number: u64,
    title: &str,
    model: Option<&str>,
    effort: Option<&str>,
    elapsed: Duration,
    budget_min: Option<u64>,
    opts: RenderOpts,
    width: usize,
) -> String {
    let icon = match phase {
        Phase::Planning => pick("🧠", "[plan]", opts.emoji),
        Phase::Executing => pick("⚙️", "[exec]", opts.emoji),
    };
    let seg = model_effort_seg(model, effort);
    // A `0` budget means the per-issue cap is disabled (unbounded, explicit opt-out):
    // show only the elapsed clock, never a misleading `/ 0:00` ceiling.
    let clock = match budget_min {
        Some(b) if b > 0 => format!(
            "{} / {}",
            fmt_clock(elapsed),
            fmt_clock(Duration::from_secs(b * 60))
        ),
        _ => fmt_clock(elapsed),
    };
    // The tail (model/effort + clock) is the fixed-cost, information-dense part
    // and is never truncated; only the elastic title gives up columns. `·`
    // (U+00B7) measures width_cjk=2, not the 1 its glyph suggests — always
    // measure the joiner, never assume its byte length.
    let join_width = fit::display_width(" · ");
    let prefix = format!("{icon} #{number} ");
    let mut overhead = fit::display_width(&prefix) + fit::display_width(&clock) + join_width;
    if let Some(s) = &seg {
        overhead += fit::display_width(s) + join_width;
    }
    let title = fit::truncate_to_width(title, width.saturating_sub(overhead));

    let mut parts: Vec<String> = vec![format!("{prefix}{title}")];
    if let Some(seg) = seg {
        parts.push(if opts.color {
            Style::new().cyan().apply_to(seg).to_string()
        } else {
            seg
        });
    }
    parts.push(if opts.color {
        Style::new().dim().apply_to(clock).to_string()
    } else {
        clock
    });
    parts.join(" · ")
}

/// Render a [`RunEvent`] to a single line, or `None` for live-region-only events.
/// The local timestamp and the outcome glyph are always present on a surfaced
/// line; colour is applied only when `opts.color` is set.
pub(crate) fn render_line(
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
        // The ADR-0019 run-boundary events and the raw plan snapshots (#96) are for
        // the CloudEvents sink only; the console already draws its own header/panel,
        // so no scroll-up line here.
        RunEvent::RunStarted { .. }
        | RunEvent::RunFinished { .. }
        | RunEvent::RunSkipped { .. }
        | RunEvent::PlanOpened { .. }
        | RunEvent::PlanClosed { .. } => return None,
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
            blockers,
        } => (
            pick("⏭️", "[skip]", opts.emoji),
            Style::new().dim(),
            format!(
                "#{number} {}{dur}",
                skip_label(*kind, label.as_deref(), blockers)
            ),
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
        RunEvent::ApiDegraded => (
            pick("🔄", "[api]", opts.emoji),
            Style::new().yellow(),
            "API degraded — child retrying".to_string(),
        ),
        RunEvent::ApiRecovered => (
            pick("🔄", "[api]", opts.emoji),
            Style::new().green(),
            "API recovered — resuming".to_string(),
        ),
        RunEvent::IdleReaped { idle_minutes } => (
            pick("💤", "[idle]", opts.emoji),
            Style::new().yellow(),
            format!("no progress for {idle_minutes} min — child reaped"),
        ),
        // The agent asking for the operator is scroll-worthy (ADR-0059 §2: a
        // `waiting` is a push); `working`/`done` are live-region only.
        RunEvent::AgentState { state, detail, .. } if state == "waiting" => (
            pick("🙋", "[agent]", opts.emoji),
            Style::new().yellow(),
            format!(
                "agent is waiting: {}",
                detail.as_deref().unwrap_or("input needed")
            ),
        ),
        RunEvent::AgentState { .. } => return None,
        RunEvent::DeadlinePassed { number } => (
            pick("⏱️", "[timeout]", opts.emoji),
            Style::new().yellow(),
            format!("deadline reached before #{number}"),
        ),
        RunEvent::RunStopped { number } => (
            pick("🛑", "[stop]", opts.emoji),
            Style::new().yellow(),
            match number {
                0 => "stopped by the operator".to_string(),
                n => format!("stopped by the operator during #{n}"),
            },
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
