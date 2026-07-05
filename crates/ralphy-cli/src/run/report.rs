//! Final-run reporting: the closing side of a run — the ADR-0006 panel assembly and
//! print, the ADR-0019 `run finished` boundary event, the ADR-0008 knowledge
//! consolidation trigger, and the small pure helpers (`outcome_of`, `empty_queue_scope`)
//! the orchestrator consults on the exit paths. All read a finished `QueueReport` (or
//! the run's tallies); none participate in run-config wiring (that lives in
//! [`super::wiring`]).

use ralphy_core::{git, BranchMode, Outcome, StopReason, Workspace};
use tracing::{info, warn};

use crate::{pricing, ui};

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
/// was already cleared up front; defaults mirror the `consolidate` command
/// (opus / medium / 30 min).
pub(crate) fn maybe_consolidate_knowledge(
    run_ok: bool,
    dry_run: bool,
    ws: &Workspace,
    stamp: &str,
) {
    if run_ok && !dry_run {
        let notes = ralphy_core::knowledge::loose_notes(ws);
        if !notes.is_empty() {
            info!(count = notes.len() as u64, "consolidating knowledge");
            let run_dir = ws.run_dir(stamp);
            match crate::run_consolidation(ws, &run_dir, Some("opus"), Some("medium"), 30, &notes) {
                Ok(archived) => info!(count = archived as u64, "knowledge consolidated"),
                Err(e) => {
                    warn!(error = %e, "knowledge consolidation failed — notes kept loose for retry")
                }
            }
        }
    }
}

/// Emit the ADR-0019 `run finished` boundary event off a clean `QueueReport`: the
/// done/skipped tallies (the generic skip bucket kept distinct from a non-green stop
/// and a human-gate park, mirroring the console panel), the run-usage token split,
/// and the run's wall-clock `duration_s` anchored on `run_start`.
pub(crate) fn emit_run_finished(
    report: &ralphy_core::QueueReport,
    queue_len: usize,
    run_start: std::time::Instant,
) {
    let issues_done = report
        .worked
        .iter()
        .filter(|r| r.outcome == Some(Outcome::Done))
        .count() as u64;
    // The generic skip bucket (a dependency/stop-before/human-return/verify
    // skip), kept distinct from a non-green stop and a human-gate park, mirrors
    // the console panel's `skipped` tally.
    let issues_skipped = report
        .worked
        .iter()
        .filter(|r| r.outcome.is_none() && r.human_blockers.is_empty())
        .count() as u64;
    let u = &report.run_usage;
    info!(
        outcome = outcome_of(&report.stop),
        issues_done,
        issues_skipped,
        issues_total = queue_len as u64,
        up = u.input,
        cr = u.cache_read,
        cw = u.cache_creation,
        out = u.output,
        duration_s = run_start.elapsed().as_secs(),
        "run finished"
    );
}

/// Assemble and print the final run panel (ADR-0006/-0008): bucket the worked issues
/// into the done/blocked/skipped/hitl triad, map the stop reason and branch mode into
/// their panel shapes, compute the run + project token totals and their read-time USD
/// (ADR-0008 D8/D11, priced per model), and hand the assembled `PanelData` to the
/// presenter. Consumes `report` (its branch/commits/undo fields move into the panel).
pub(crate) fn render_final_panel(
    presenter: &ui::PresenterHandle,
    report: ralphy_core::QueueReport,
    branch_mode: BranchMode,
    dry_run: bool,
    repo_root: &std::path::Path,
) {
    // Bucket the worked issues into the three-way triad defined in the plan.
    let done = report
        .worked
        .iter()
        .filter(|r| r.outcome == Some(Outcome::Done))
        .count() as u64;
    let num_blocked = report
        .worked
        .iter()
        .filter(|r| r.outcome.is_some() && r.outcome != Some(Outcome::Done))
        .count() as u64;
    // Issues stalled on a human gate in their path (ADR-0014) get their own
    // bucket and are kept out of the generic skipped tally, mirroring how the
    // live card gives them a distinct status.
    let hitl = report
        .worked
        .iter()
        .filter(|r| r.outcome.is_none() && !r.human_blockers.is_empty())
        .count() as u64;
    let skipped = report
        .worked
        .iter()
        .filter(|r| r.outcome.is_none() && r.human_blockers.is_empty())
        .count() as u64;

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
    let to_lite = |u: &ralphy_core::Usage| ui::UsageLite {
        input: u.input,
        cache_read: u.cache_read,
        cache_creation: u.cache_creation,
        output: u.output,
        model: None,
    };

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
        done,
        blocked: num_blocked,
        skipped,
        hitl,
        commits: report.commits,
        stop: panel_stop,
        branch_mode: panel_mode,
        dry_run,
        undo_tag: report.undo_tag,
        run_breakdown: to_lite(run_usage),
        project_breakdown: to_lite(&project_usage),
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
