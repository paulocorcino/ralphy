use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};
use ralphy_core::{DraftRequest, IssuesMode, Workspace};

use crate::init::issues::{
    decide_issues_path, draft_decision, draft_with_agent, format_draft_summary, load_issues_draft,
    publish_decision, publish_draft, resolve_triage_label, ChosenAgent, IssuesPath,
};
use crate::init::render::{
    ask_yes_no, print_bullet, print_note, print_ok, print_section, with_spinner,
};
use crate::init::wizard::{InitConfig, InitState, Stage};

/// Stage 8: backlog/milestone → issues (preview, confirm, publish).
pub(in crate::init::run) fn issues_stage(
    repo: &Path,
    ws: &Workspace,
    state: &mut InitState,
    cfg: &InitConfig,
    chosen: ChosenAgent<'_>,
) -> Result<()> {
    if state.is_done(Stage::Issues) {
        note_issues_done(state);
        return Ok(());
    }

    // Partial-publish resume: a prior run already created the milestone and/or
    // some issues but did not finish (a transient `gh` error mid-loop, or a
    // crash). The persisted `issues-draft.json` is the draft those numbers — and
    // the created milestone title — correspond to, so RELOAD it and publish only
    // the remainder. We must NOT re-draft here: a regenerated draft could reorder
    // the prefix (making `skip(created_issues.len())` recreate an already-
    // published issue) or carry a different milestone title than the one already
    // on GitHub. A clean re-draft happens only below, when nothing was published
    // yet (no milestone created and no issues created).
    if !state.created_issues.is_empty() || state.milestone_created.is_some() {
        return publish_remaining(repo, ws, state);
    }

    let source = match decide_issues_path(cfg) {
        IssuesPath::Skip => {
            print_section(
                "First tasks",
                Some("No backlog or planning docs found — skipping task creation."),
            );
            // Nothing to publish — the stage completed; record it so a re-run
            // doesn't reconsider an empty backlog.
            state.mark(Stage::Issues);
            return state.save(ws);
        }
        IssuesPath::Milestone => (IssuesMode::Milestone, cfg.milestone_docs.clone()),
        IssuesPath::LooseBacklog => (
            IssuesMode::LooseBacklog,
            cfg.backlog_location.iter().cloned().collect(),
        ),
    };
    draft_and_publish(repo, ws, state, source, chosen)
}

fn note_issues_done(state: &InitState) {
    if state.created_issues.is_empty() {
        print_note("Resuming: issue stage already done — nothing was published.");
        return;
    }

    let nums: Vec<String> = state
        .created_issues
        .iter()
        .map(|n| format!("#{n}"))
        .collect();
    print_note(&format!(
        "Resuming: issues already published ({}).",
        nums.join(", ")
    ));
}

fn publish_remaining(repo: &Path, ws: &Workspace, state: &mut InitState) -> Result<()> {
    let draft_path = ws.issues_draft_path();
    if !draft_path.exists() {
        bail!(
            "ralphy init: resume recorded {} published issue(s) but the draft at {} is gone, \
             so the remaining issues can't be published safely — delete \
             .ralphy/init-state.json to restart issue creation from scratch",
            state.created_issues.len(),
            draft_path.display()
        );
    }

    let draft = load_issues_draft(&draft_path)?;
    // Guard against a tampered/truncated draft: it must hold at least the
    // already-published prefix, else `skip` would silently drop the remainder
    // and we'd mark the stage done having published nothing more.
    if draft.issues.len() < state.created_issues.len() {
        bail!(
            "ralphy init: the draft at {} has {} issue(s) but {} were already published — \
             it no longer matches the checkpoint; delete .ralphy/init-state.json to restart \
             issue creation from scratch",
            draft_path.display(),
            draft.issues.len(),
            state.created_issues.len()
        );
    }

    print_note(&format!(
        "Resuming publish: {} issue(s) already created; publishing the rest from {}…",
        state.created_issues.len(),
        draft_path.display()
    ));
    with_spinner("Publishing remaining issues…", || {
        publish_draft(repo, &draft, state, ws)
    })?;
    print_ok(&format!(
        "Published {} issue(s) total.",
        state.created_issues.len()
    ));
    state.mark(Stage::Issues);
    state.save(ws)
}

fn draft_and_publish(
    repo: &Path,
    ws: &Workspace,
    state: &mut InitState,
    (mode, source_docs): (IssuesMode, Vec<String>),
    chosen: ChosenAgent<'_>,
) -> Result<()> {
    print_section(
        "First tasks",
        Some("Ralphy can read your docs to draft a first set of tasks (nothing is published yet)."),
    );
    let answer = ask_yes_no("Draft a first set of tasks from your docs?", true)?;
    if !draft_decision(&answer) {
        // Declined — don't run the agent. Leave Stage::Issues unmarked so a
        // re-run offers drafting again (mirrors a declined publish).
        print_note("Skipped — no tasks drafted.");
        return Ok(());
    }

    let triage_label = resolve_triage_label(repo);
    let draft_path = ws.issues_draft_path();
    let req = DraftRequest {
        mode,
        source_docs: &source_docs,
        triage_label: &triage_label,
    };
    let draft = with_spinner("Drafting tasks…", || {
        draft_with_agent(chosen, repo, &draft_path, &req, Duration::from_secs(600))
    })?;
    print_note(&format!("Draft written to {}", draft_path.display()));

    outln!();
    for l in format_draft_summary(&draft).lines() {
        print_bullet(l);
    }

    let answer = ask_yes_no("Publish these tasks as issues on GitHub?", false)?;
    if publish_decision(&answer) {
        with_spinner("Publishing issues…", || {
            publish_draft(repo, &draft, state, ws)
        })?;
        print_ok(&format!(
            "Published {} issue(s).",
            state.created_issues.len()
        ));
        state.mark(Stage::Issues);
        state.save(ws)?;
    } else {
        // A declined publish leaves the draft on disk; do NOT mark Issues
        // done, so a re-run still offers to publish it (per Decisions).
        print_note(&format!(
            "Skipped — draft kept at {}.",
            draft_path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
