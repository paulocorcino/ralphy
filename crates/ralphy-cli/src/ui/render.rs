//! Stateless render formatters: pure functions and DTOs that turn a
//! [`RunEvent`](crate::runstate::RunEvent) or panel data into display strings.
//! No state, no `indicatif` — the live-region machine lives in
//! [`presenter`](super::presenter).

use std::time::Duration;

use chrono::{DateTime, Local};
use console::Style;
use tracing::Level;

use super::{fit, FinishOutcome, Phase, UsageLite};
use crate::pricing::PriceTable;
use crate::runstate::{RunEvent, SkipKind};

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

/// `MM:SS` clock form (minutes may exceed 59), e.g. `12:43`, `45:00`. The
/// active-line/budget form; distinct from [`fmt_duration`]'s `2m13s` finished-line
/// form.
pub(crate) fn fmt_clock(d: Duration) -> String {
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
    /// The end-of-run knowledge-consolidation pass's own token breakdown, shown as
    /// a distinct footer segment so this run overhead stays legible next to the run
    /// total it is folded into (issue #269). `None` when the pass did not run.
    pub consolidate_breakdown: Option<UsageLite>,
    /// Read-time USD for the consolidation segment (ADR-0008 D8). `None` when the
    /// pass did not run or its model is unpriced.
    pub consolidate_usd: Option<f64>,
    /// The read-time harvest-tax ESTIMATE for a harvesting vendor (issue #270):
    /// `harvest_floor × invocation_count` input tokens the vendor's CLI injected by
    /// auto-discovering foreign skills, plus the invocation count for the `(N× ~Mk)`
    /// gloss. `None` for a non-harvesting vendor (the segment is omitted). It is an
    /// input-side estimate, never a priced/stored figure — the analog of `run_usd`,
    /// which is likewise derived read-time and never entered on the ledger.
    pub harvest_est: Option<HarvestEst>,
}

/// The read-time harvest-tax estimate (issue #270): `Some(floor × invocations)`
/// when the vendor harvests (`floor.is_some()`) and at least one invocation was
/// counted, else `None` — a non-harvesting vendor or an unknown/zero count omits the
/// segment entirely rather than rendering a nonsensical `0×`. The one place the
/// estimate arithmetic lives, shared by the per-issue done line and the run footer.
pub(crate) fn harvest_est(floor: Option<u64>, invocations: Option<u64>) -> Option<HarvestEst> {
    let floor = floor?;
    let invocations = invocations.filter(|&n| n > 0)?;
    Some(HarvestEst {
        tokens: floor.saturating_mul(invocations),
        invocations,
        floor,
    })
}

/// The per-run harvest-tax estimate for the footer (issue #270): the estimated
/// injected input tokens and the invocation count they were derived from.
#[derive(Debug, Clone, Copy)]
pub struct HarvestEst {
    /// `harvest_floor × invocations` — the estimated input tokens injected across
    /// the run by the vendor's foreign-skill harvest.
    pub tokens: u64,
    /// The invocation count the estimate multiplied the floor by, for the `(N× …)`
    /// gloss so the operator can see the arithmetic.
    pub invocations: u64,
    /// The per-invocation floor, for the `(N× ~Mk)` gloss.
    pub floor: u64,
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
    // The consolidation segment (issue #269): shown only on a run that consolidated,
    // between the run total it is part of and the project balance. `false` for the
    // partial flag — the segment prices a single model, so there is no priced/unpriced
    // split to flag; an unpriced model already renders `$?`.
    let consolidate_seg = match &data.consolidate_breakdown {
        Some(u) => format!(
            " · consolidate: {}",
            fmt_breakdown(u, data.consolidate_usd, false, opts.emoji)
        ),
        None => String::new(),
    };
    // The harvest-tax ESTIMATE segment (issue #270): shown only for a harvesting
    // vendor (Cursor today), between the run/consolidate figures it is folded into
    // and the project balance. Tokens only, labelled `est` — it is a read-time
    // projection like USD, never a stored or priced figure.
    let harvest_seg = match &data.harvest_est {
        Some(est) => format!(" · harvest est: {}", fmt_harvest_est(est, opts.emoji)),
        None => String::new(),
    };
    let footer_raw = format!(
        "run: {}{}{} · project: {} {}",
        fmt_breakdown(
            &data.run_breakdown,
            data.run_usd,
            data.run_usd_partial,
            opts.emoji
        ),
        consolidate_seg,
        harvest_seg,
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
pub(crate) struct Meter {
    pub(crate) usage: UsageLite,
    pub(crate) usd: Option<f64>,
    pub(crate) partial: bool,
}

/// Per-line render context the presenter computes in `drive` (it owns the clock,
/// the active issue's display model/effort, and the price table) and hands to
/// `render_line`. All fields are absent for events that carry no meter/duration.
#[derive(Default)]
pub(crate) struct LineExtra {
    pub(crate) duration: Option<Duration>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) meter: Option<Meter>,
    /// The per-issue harvest-tax estimate (issue #270), present only on the `done`
    /// line of a harvesting vendor's issue. `None` elsewhere (the segment is omitted).
    pub(crate) harvest_est: Option<HarvestEst>,
}

/// Price one phase's [`UsageLite`] at read time, or `None` when its model is absent
/// or unpriced. `UsageLite` aliases the core `Usage` the [`PriceTable`] prices on.
fn price_lite(pt: &PriceTable, u: &UsageLite) -> Option<f64> {
    let model = u.model.as_deref().filter(|m| !m.is_empty())?;
    pt.cost_usd(model, u)
}

/// Build a [`Meter`] for an issue line from its planning usage (stashed, may be
/// absent on the `plan written` line) and a final phase's usage. The display
/// breakdown sums both phases; the USD prices each phase's model separately (plan
/// and execute often differ) and sums the priced portion, mirroring
/// `cost_usd_by_model`'s `$?`/`+?` semantics (ADR-0008 D8).
pub(crate) fn meter_for(pt: &PriceTable, plan: Option<&UsageLite>, last: &UsageLite) -> Meter {
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
pub(crate) fn fmt_usd_compact(usd: Option<f64>, partial: bool) -> String {
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
    if let Some(est) = extra.harvest_est.as_ref() {
        tail.push(format!("harvest est {}", fmt_harvest_est(est, opts.emoji)));
    }
    if tail.is_empty() {
        format!("#{number} {label}")
    } else {
        format!("#{number} {label} · {}", tail.join(" · "))
    }
}

/// Format a harvest-tax estimate (issue #270): `~↑47.0k (3× ~15.7k)` — the estimated
/// injected input tokens, then the `(invocations× ~floor)` gloss so the operator sees
/// the arithmetic. The leading `~` and the `est` label a caller prepends both mark it a
/// projection, never a measured/priced figure. ASCII path uses `in ` for the glyph.
fn fmt_harvest_est(est: &HarvestEst, emoji: bool) -> String {
    let up = if emoji { "↑" } else { "in " };
    format!(
        "~{up}{} ({}× ~{})",
        fmt_tokens(est.tokens),
        est.invocations,
        fmt_tokens(est.floor)
    )
}

/// Format a token count compactly for the footer: `1.2M`, `8.4k`, or a bare
/// `912` under a thousand. One decimal place for the scaled forms.
pub(crate) fn fmt_tokens(n: u64) -> String {
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
pub(crate) fn pick(emoji: &'static str, ascii: &'static str, use_emoji: bool) -> &'static str {
    if use_emoji {
        emoji
    } else {
        ascii
    }
}

/// `13s` or `2m05s`.
pub(crate) fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}
