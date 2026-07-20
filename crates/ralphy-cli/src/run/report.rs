//! Final-run reporting: the closing side of a run — the ADR-0006 panel assembly and
//! print, the ADR-0019 `run finished` boundary event, the ADR-0008 knowledge
//! consolidation trigger, and the small pure helpers (`outcome_of`, `empty_queue_scope`)
//! the orchestrator consults on the exit paths. All read a finished `QueueReport` (or
//! the run's tallies); none participate in run-config wiring (that lives in
//! [`super::wiring`]).

use ralphy_core::{git, BranchMode, StopReason, Workspace};
use tracing::warn;

use super::summary::RunSummary;
use crate::{pricing, ui, CliAgent};

/// Knowledge consolidation trigger: a non-dry run that finished (`run_ok`) and left
/// loose per-issue notes folds them into KNOWLEDGE.md, so the curated cache the next
/// run reads (prompt.execute.md reads KNOWLEDGE.md first) stays current without a
/// manual `consolidate` step. Everything lives under the gitignored `.ralphy/`, so
/// there is nothing to commit and the panel's "clean run" report stays accurate. The
/// caller runs this AFTER the presenter finalize and BEFORE the notifier/sink
/// shutdown so it surfaces as a first-class lifecycle event in both surfaces: the
/// `info!`/`warn!` here decode to RunEvents the console presenter renders
/// (timestamp + 📚) and the live Telegram card folds (a 📚 line during, a footer
/// segment after). A failed session is a warning, never a run failure — the run
/// already succeeded and the notes stay loose for a later retry. `ANTHROPIC_API_KEY`
/// was already cleared up front. The session runs on the run's own executor
/// `agent` (docs/adr/0031) — not a hardwired Claude — so a Kimi/Codex/OpenCode run
/// never reaches for `claude`. The model/effort defaults come from
/// `consolidate_defaults`: opus/medium for Claude (curation is judgment-heavy),
/// the adapter's own default for the rest. 30-minute wall like the command.
pub(crate) fn maybe_consolidate_knowledge(
    agent: CliAgent,
    run_ok: bool,
    dry_run: bool,
    ws: &Workspace,
    stamp: &str,
) {
    if run_ok && !dry_run {
        let notes = ralphy_core::knowledge::loose_notes(ws);
        if !notes.is_empty() {
            ralphy_core::emit::knowledge_consolidating(notes.len() as u64);
            let run_dir = ws.run_dir(stamp);
            let (model, effort) = crate::consolidate_defaults(agent);
            match crate::run_consolidation(agent, ws, &run_dir, model, effort, 30, &notes) {
                Ok(archived) => ralphy_core::emit::knowledge_consolidated(archived as u64),
                Err(e) => {
                    warn!(error = %e, "knowledge consolidation failed — notes kept loose for retry")
                }
            }
        }
    }
}

/// Emit the ADR-0019 `run finished` boundary event off the run's [`RunSummary`] —
/// the SAME fold the final panel prints, so the event's tallies and the console's
/// can never disagree. Carries the four bucket counts, the per-issue rollup, the
/// run-usage token split, and the wall-clock `duration_s` anchored on `run_start`.
pub(crate) fn emit_run_finished(
    summary: &RunSummary,
    run_usage: &ralphy_core::Usage,
    run_start: std::time::Instant,
) {
    ralphy_core::emit::run_finished(
        summary.outcome,
        summary.done,
        summary.skipped,
        summary.total,
        summary.blocked,
        summary.hitl,
        &summary.issues_json(),
        run_usage,
        run_start.elapsed().as_secs(),
    );
}

/// Emit the ADR-0019 `run finished` boundary event for the EMPTY-QUEUE border (#222):
/// the `no_work` outcome, every count at 0, no usage. Separate from
/// [`emit_run_finished`] because that one maps a `StopReason` off a real
/// `QueueReport` — an empty run has none, and threading a synthetic one through
/// `finalize_run` would also trigger `maybe_consolidate_knowledge`, i.e. spawn a paid
/// consolidation session on a run that did no work.
pub(crate) fn emit_run_finished_no_work(run_start: std::time::Instant) {
    ralphy_core::emit::run_finished(
        "no_work",
        0,
        0,
        0,
        0,
        0,
        "",
        &ralphy_core::Usage::default(),
        run_start.elapsed().as_secs(),
    );
}

/// Assemble and print the final run panel (ADR-0006/-0008): read the
/// done/blocked/skipped/hitl triad off the run's [`RunSummary`] (the same fold
/// `run.finished` publishes), map the stop reason and branch mode into their panel
/// shapes, compute the run + project token totals and their read-time USD (ADR-0008
/// D8/D11, priced per model), and hand the assembled `PanelData` to the presenter.
/// Consumes `report` (its branch/commits/undo fields move into the panel).
pub(crate) fn render_final_panel(
    presenter: &ui::PresenterHandle,
    report: ralphy_core::QueueReport,
    summary: &RunSummary,
    branch_mode: BranchMode,
    dry_run: bool,
    repo_root: &std::path::Path,
) {
    let panel_stop = report.stop.map(|s| match s {
        StopReason::Deadline => ui::PanelStop::Deadline,
        StopReason::NonGreen { number, outcome } => ui::PanelStop::NonGreen {
            number,
            outcome: format!("{outcome:?}"),
        },
        StopReason::StopBefore { number } => ui::PanelStop::StopBefore { number },
        StopReason::Limit { number, reset } => ui::PanelStop::Limit { number, reset },
    });

    let panel_mode = match branch_mode {
        BranchMode::New => ui::PanelBranchMode::New,
        BranchMode::Current => ui::PanelBranchMode::Current,
    };

    // Token-usage footer figures (ADR-0008 D11): the run total off this run's
    // accumulated usage, and the project's cumulative balance read from the ledger.
    let slug = git::project_slug(repo_root);
    let run_usage = &report.run_usage;
    let project_usage = ralphy_core::ledger::project_total(&slug);

    // Read-time USD (ADR-0008 D8), priced per model and summed. The run total
    // prices `report.run_usage_by_model` (the runner's per-model split); the
    // project total groups the cumulative ledger rows by model and prices each.
    // USD never enters the ledger — re-pricing the table re-prices history.
    let price_table = pricing::PriceTable::load();
    let (run_usd, run_partial) = price_table.cost_usd_by_model(&report.run_usage_by_model);
    let mut project_by_model: std::collections::BTreeMap<String, ralphy_core::Usage> =
        std::collections::BTreeMap::new();
    for row in ralphy_core::read_project_rows(&slug) {
        project_by_model
            .entry(row.model.clone())
            .or_default()
            .add_tokens(&row.tokens);
    }
    let (project_usd, project_partial) = price_table.cost_usd_by_model(&project_by_model);

    let data = ui::PanelData {
        branch: report.branch,
        orig_branch: report.orig_branch,
        done: summary.done,
        blocked: summary.blocked,
        skipped: summary.skipped,
        hitl: summary.hitl,
        commits: report.commits,
        stop: panel_stop,
        branch_mode: panel_mode,
        dry_run,
        undo_tag: report.undo_tag,
        // The footer meter reads only the four numeric fields; clear `model`
        // (USD is priced separately per model above) to keep display identical.
        run_breakdown: ralphy_core::Usage {
            model: None,
            ..run_usage.clone()
        },
        project_breakdown: ralphy_core::Usage {
            model: None,
            ..project_usage.clone()
        },
        project_id: slug,
        run_usd,
        project_usd,
        run_usd_partial: run_partial,
        project_usd_partial: project_partial,
    };
    presenter.print_panel(&data);
}

/// Map a queue's [`StopReason`] to the `run.finished` `outcome` label (ADR-0019).
/// `None` (the whole queue was worked) is `completed`; a usage-limit stop has no
/// `outcome` value in the contract enum, so it collapses to `non_green` — a
/// usage-limit stop is a non-clean completion (docs/events.md `run.finished`).
pub(crate) fn outcome_of(stop: &Option<StopReason>) -> &'static str {
    match stop {
        None => "completed",
        Some(StopReason::NonGreen { .. }) => "non_green",
        Some(StopReason::Deadline) => "deadline",
        Some(StopReason::StopBefore { .. }) => "stop_before",
        Some(StopReason::Limit { .. }) => "non_green",
    }
}

/// Build the human-readable scope phrase for the "No open issues for …" notice.
/// An explicit `--issues` selection or `--only-issue` names the numbers; a label
/// queue names the labels and, when an assignee filter is active, appends
/// `assigned to <login>` so the empty notice reveals the filter (ADR-0021,
/// criterion 7). `--only-issue`/`--issues` bypass the filter, so `assignee` is
/// only ever appended on the labels path.
pub(crate) fn empty_queue_scope(
    issues: &[u64],
    only_issue: Option<u64>,
    labels: &[String],
    assignee: Option<&str>,
) -> String {
    if !issues.is_empty() {
        let list = issues
            .iter()
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("issues [{list}]");
    }
    match only_issue {
        Some(n) => format!("issue #{n}"),
        None => {
            let base = format!("labels [{}]", labels.join(", "));
            match assignee {
                Some(a) => format!("{base} assigned to {a}"),
                None => base,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralphy_core::Outcome;

    #[test]
    fn outcome_of_maps_every_stop_reason() {
        assert_eq!(outcome_of(&None), "completed");
        assert_eq!(
            outcome_of(&Some(StopReason::NonGreen {
                number: 1,
                outcome: Outcome::Stuck,
            })),
            "non_green"
        );
        assert_eq!(outcome_of(&Some(StopReason::Deadline)), "deadline");
        assert_eq!(
            outcome_of(&Some(StopReason::StopBefore { number: 2 })),
            "stop_before"
        );
        // A usage-limit stop has no `outcome` value in the contract enum, so it
        // collapses to non_green (a non-clean completion).
        assert_eq!(
            outcome_of(&Some(StopReason::Limit {
                number: 3,
                reset: Some("14:30".into()),
            })),
            "non_green"
        );
    }

    #[test]
    fn empty_queue_scope_names_the_filter() {
        // Active filter on a label queue names the assignee.
        let scope = empty_queue_scope(&[], None, &["ready-for-agent".to_string()], Some("@me"));
        assert!(scope.contains("@me"), "scope must name the filter: {scope}");
        assert!(scope.contains("assigned to"), "got: {scope}");

        // No filter omits the "assigned to" phrase.
        let scope = empty_queue_scope(&[], None, &["ready-for-agent".to_string()], None);
        assert!(
            !scope.contains("assigned to"),
            "unfiltered scope must not mention an assignee: {scope}"
        );

        // Explicit selections never carry the filter phrase.
        let scope = empty_queue_scope(&[5, 3], None, &[], None);
        assert_eq!(scope, "issues [#5, #3]");
        let scope = empty_queue_scope(&[], Some(7), &["ready-for-agent".to_string()], None);
        assert_eq!(scope, "issue #7");
    }
}
