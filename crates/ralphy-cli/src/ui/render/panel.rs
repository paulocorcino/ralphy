//! The end-of-run totals panel and its data.

use console::Style;

use super::meter::fmt_meter;
use super::{pick, Meter, RenderOpts};
use crate::ui::UsageLite;

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
    NonGreen {
        number: u64,
        outcome: String,
    },
    StopBefore {
        number: u64,
    },
    Limit {
        number: u64,
        reset: Option<String>,
    },
    /// The operator asked the run to stop; `None` means it landed between issues.
    Stopped {
        number: Option<u64>,
    },
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
    /// The issue numbers that closed carrying `[review-only]` ledger lines,
    /// folded in `run/summary.rs` (#313) — numbers only, never ledger data, so
    /// the renderer cannot re-derive the predicate. Empty on an ordinary run.
    pub review_only: Vec<u64>,
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
}

/// Render the end-of-run totals panel as a `Vec<String>` of lines ready to
/// `println!`. Produces: a counts line (`✅/⛔/⏭️`), an optional `🙋 waiting on
/// human` line, an optional review-debt line (#313, only when `review_only` is
/// non-empty), a commits line, an optional
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

    // Review debt (#313) — an attribute of DONE issues, so it never touches the
    // counts line above and stays out of the yellow "waiting on human" bucket.
    if !data.review_only.is_empty() {
        let review_icon = pick("🔎", "[review]", opts.emoji);
        let n = data.review_only.len();
        let (noun, verb) = if n == 1 {
            ("issue", "needs")
        } else {
            ("issues", "need")
        };
        let numbers = data
            .review_only
            .iter()
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let review_raw = format!(
            "{review_icon} {n} {noun} {verb} your eyes: {numbers} \
             — review-only criteria, delivered but not machine-checked"
        );
        lines.push(if opts.color {
            Style::new().cyan().apply_to(&review_raw).to_string()
        } else {
            review_raw
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
            // Both arms say what happened to the WORK, because that is the only
            // thing the operator cannot see from having pressed the button.
            PanelStop::Stopped { number: Some(n) } => {
                format!("Stopped: you stopped the run during #{n}. Branch handed back with the work in place; #{n} stays open.")
            }
            PanelStop::Stopped { number: None } => {
                "Stopped: you stopped the run between issues. Branch handed back.".to_string()
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
    let footer_raw = format!(
        "run: {}{} · project: {} {}",
        fmt_breakdown(
            &data.run_breakdown,
            data.run_usd,
            data.run_usd_partial,
            opts.emoji
        ),
        consolidate_seg,
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
