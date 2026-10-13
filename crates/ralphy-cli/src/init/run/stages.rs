//! The stages of `ralphy init`, one function per stage of the checkpoint
//! (`InitState`). [`super::run`] calls them in order; each stage reads and
//! writes the checkpoint itself, so a re-run resumes where the last one ended.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ralphy_core::{git, github, DiagnosisReport, DraftRequest, IssuesMode, Workspace};

use super::decisions::{
    branch_decision, commit_decision, labels_decision, BranchDecision, CommitDecision,
};
use super::{diagnose_with_agent, diagnosis_cwd};
use crate::init::gate::{
    agent_logged_in, agent_present, evaluate_gate, gh_authenticated, git_present, github_remote,
    python_present, Agent, EnvFindings,
};
use crate::init::issues::{
    decide_issues_path, draft_decision, draft_with_agent, format_draft_summary, load_issues_draft,
    publish_decision, publish_draft, resolve_triage_label, IssuesPath,
};
use crate::init::render::{
    ask_yes_no, print_bullet, print_captured_config, print_gate_report, print_note, print_ok,
    print_section, run_qa, with_spinner,
};
use crate::init::scaffold::write_scaffold;
use crate::init::skills::{
    download_decision, install_skills_step, skill_names, skills_target, sparse_fetch_commands,
    Outcome, SKILLS_REF, SKILLS_SUBTREE,
};
use crate::init::wizard::{persist_report, InitConfig, InitState, Stage};

/// Open `init` with the same face as the run command: the `🦊 Ralphy - vX`
/// header + `📦 project · 🌿 branch · 🔗 url` info line. The face is seeded with
/// the repo name so it is stable for this repo; the info-line segments are
/// best-effort (a detached HEAD or local-only repo drops a part).
pub(super) fn show_banner(repo: &Path) {
    let repo_name = repo.file_name().and_then(|s| s.to_str()).unwrap_or("repo");
    let banner = crate::ui::Presenter::new().handle();
    banner.print_header(repo_name);
    let branch = git::current_branch(repo).ok();
    let url = git::origin_url(repo).map(|u| crate::ui::normalize_remote_url(&u));
    banner.print_info_line(repo_name, branch.as_deref(), url.as_deref());
    // Tear down the banner's live region now: init has no progress bars, and
    // leaving it active swallows the blank line the gate section prints next.
    banner.finalize();
}

/// Run the environment gate and stop when it has a blocker.
pub(super) fn check_environment(repo: &Path) -> Result<EnvFindings> {
    // Run the (subprocess-backed: `gh auth status`, agent `whoami`/login probes)
    // environment checks behind a spinner so the multi-second wait shows life.
    let findings = with_spinner("Analyzing the environment…", || {
        let agents_present: Vec<Agent> = Agent::ALL.iter().copied().filter(agent_present).collect();
        let agents_logged_in: Vec<Agent> = agents_present
            .iter()
            .copied()
            .filter(agent_logged_in)
            .collect();
        EnvFindings {
            git: git_present(),
            python: python_present(),
            gh_authenticated: gh_authenticated(),
            github_remote: github_remote(repo),
            agents_present,
            agents_logged_in,
        }
    });

    let fails = evaluate_gate(&findings);
    print_gate_report(&findings, &fails);

    if !fails.is_empty() {
        bail!(
            "ralphy init: environment gate failed ({} blocker(s)) — see report above",
            fails.len()
        );
    }
    Ok(findings)
}

/// Stages 2–3: the diagnosis and the Q&A. The captured config is the resume
/// key: present ⇒ the costly agent diagnosis and the interactive Q&A already
/// ran, so skip both.
pub(super) fn capture_config(
    repo: &Path,
    ws: &Workspace,
    state: &mut InitState,
    agent: Agent,
    model: Option<&str>,
) -> Result<InitConfig> {
    let cfg = if let Some(cfg) = state.config.clone() {
        print_note("Resuming: diagnosis + Q&A already captured — skipping.");
        cfg
    } else {
        let report = load_or_run_diagnosis(repo, ws, agent, model)?;
        let cfg = confirm_diagnosis(&report)?;
        state.config = Some(cfg.clone());
        state.mark(Stage::Diagnose);
        state.save(ws)?;
        cfg
    };

    print_section("Captured config", None);
    print_captured_config(&cfg);
    Ok(cfg)
}

/// Reload a persisted report when one exists (a crash during the interactive
/// Q&A left `diagnosis.json` but no config), otherwise run the diagnosis from a
/// neutral cwd OUTSIDE the repo — so the target's CLAUDE.md/AGENTS.md are read
/// as data, never auto-loaded as instructions.
fn load_or_run_diagnosis(
    repo: &Path,
    ws: &Workspace,
    agent: Agent,
    model: Option<&str>,
) -> Result<DiagnosisReport> {
    if ws.diagnosis_path().exists() {
        print_note(&format!(
            "Reusing persisted diagnosis at {}.",
            ws.diagnosis_path().display()
        ));
        let raw = std::fs::read_to_string(ws.diagnosis_path())
            .with_context(|| format!("reading {}", ws.diagnosis_path().display()))?;
        return serde_json::from_str(&raw)
            .with_context(|| format!("parsing {}", ws.diagnosis_path().display()));
    }

    let stamp = format!("{}", std::process::id());
    let cwd = diagnosis_cwd(repo, &stamp);
    let report = with_spinner("Scanning your repo (read-only)…", || {
        diagnose_with_agent(
            agent,
            repo,
            &cwd,
            model,
            Some("medium"),
            Duration::from_secs(300),
        )
    })?;
    persist_report(ws, &report)?;
    print_note(&format!(
        "Diagnosis written to {}",
        ws.diagnosis_path().display()
    ));
    Ok(report)
}

/// Console Q&A pre-filled by the diagnosis — the dev confirms/corrects.
fn confirm_diagnosis(report: &DiagnosisReport) -> Result<InitConfig> {
    print_section(
        "Confirm the diagnosis",
        Some(
            "Ralphy pre-filled these 8 fields from its read-only scan. For each one: press \
             Enter to keep the value shown, type a new value to change it, or 'none' to clear \
             an optional field.",
        ),
    );
    run_qa(report)
}

/// Stage 4: git safety (snapshot commit) and the setup branch.
pub(super) fn git_stage(repo: &Path, ws: &Workspace, state: &mut InitState) -> Result<()> {
    if state.is_done(Stage::Git) {
        print_note("Resuming: git safety + branch already done — skipping.");
        return Ok(());
    }

    save_uncommitted_changes(repo)?;
    switch_to_setup_branch(repo)?;

    state.mark(Stage::Git);
    state.save(ws)
}

fn save_uncommitted_changes(repo: &Path) -> Result<()> {
    let is_clean = git::is_clean_ignoring_ralphy(repo)?;
    if is_clean {
        return Ok(());
    }

    print_section(
        "Git safety",
        Some("You have uncommitted changes. Ralphy can save them in a commit first."),
    );
    for l in git::git(repo, &["status", "--short"])?.lines() {
        print_bullet(l.trim_end());
    }

    let answer = ask_yes_no("Save your current changes in a commit first?", true)?;
    match commit_decision(is_clean, &answer) {
        CommitDecision::Abort(msg) => {
            // INVARIANT: a refusal stops here — before any branch or write.
            bail!("{msg}");
        }

        CommitDecision::Commit => {
            git::commit_all_snapshot(repo)?;
            print_ok("Changes committed.");
        }

        CommitDecision::NothingToCommit => {}
    }
    Ok(())
}

/// Stage 4b: branch (before any scaffold write).
fn switch_to_setup_branch(repo: &Path) -> Result<()> {
    let current = git::current_branch(repo)?;
    let answer = ask_yes_no("Do Ralphy's setup on a new branch `ralphy/init`?", true)?;
    match branch_decision(&current, &answer) {
        BranchDecision::Create(branch) => {
            if git::commitish_exists(repo, &branch) {
                git::checkout(repo, &branch)?;
            } else {
                git::checkout_new_branch(repo, &branch, &current)?;
            }

            print_ok(&format!("Working on branch {branch}."));
        }

        BranchDecision::Stay => {
            print_ok(&format!("Staying on branch {current}."));
        }
    }
    Ok(())
}

/// Stage 5: deterministic scaffold (onto the branch).
pub(super) fn scaffold_stage(
    repo: &Path,
    ws: &Workspace,
    state: &mut InitState,
    cfg: &InitConfig,
) -> Result<()> {
    if state.is_done(Stage::Scaffold) {
        print_note("Resuming: scaffold already written — skipping.");
        return Ok(());
    }

    write_scaffold(repo, cfg)?;
    print_section(
        "Project files",
        Some("Created starter docs for the agent to use:"),
    );
    print_bullet("docs/agents/issue-tracker.md");
    print_bullet("docs/agents/triage-labels.md");
    print_bullet("docs/agents/domain.md");
    if cfg.adopt_prd_roadmap {
        print_bullet("docs/roadmap.md");
        print_bullet("docs/prd/README.md");
        print_bullet("docs/prd/_template.md");
    }

    state.mark(Stage::Scaffold);
    state.save(ws)
}

/// Stage 6: download engineering skills.
pub(super) fn skills_stage(
    repo: &Path,
    ws: &Workspace,
    state: &mut InitState,
    cfg: &InitConfig,
) -> Result<()> {
    if state.is_done(Stage::Skills) {
        print_note("Resuming: skills step already done — skipping.");
        return Ok(());
    }

    offer_skills(repo, cfg)?;

    state.mark(Stage::Skills);
    state.save(ws)
}

fn offer_skills(repo: &Path, cfg: &InitConfig) -> Result<()> {
    let names = skill_names();
    let skills_dst = repo.join(skills_target(cfg.skills_dir.as_deref()));
    // NOTE: displayed list is the static INSTALLABLE_SKILLS; the downloaded set
    // is whatever `main` of the skills repo holds (see SKILLS_REF) and may drift.
    print_section(
        "Agent skills",
        Some(&format!(
            "Optional ready-made skills, installed into {}:",
            skills_dst.display()
        )),
    );
    for name in &names {
        print_bullet(name);
    }

    let answer = ask_yes_no("Install these skills?", false)?;
    if !download_decision(&answer) {
        print_note("Skipped — no skills installed.");
        return Ok(());
    }
    install_skills(&skills_dst)
}

fn install_skills(skills_dst: &Path) -> Result<()> {
    let fetch_ref = SKILLS_REF.to_string();
    let subtree = SKILLS_SUBTREE.to_string();
    let fetch = |scratch: &Path| -> Result<PathBuf> {
        for argv in sparse_fetch_commands(&fetch_ref, &subtree) {
            let args: Vec<&str> = argv.iter().map(String::as_str).collect();
            git::git(scratch, &args)?;
        }

        Ok(scratch.join(&subtree))
    };
    let outcome = with_spinner("Installing skills…", || {
        install_skills_step(skills_dst, fetch)
    })?;
    match outcome {
        Outcome::Installed(n) => print_ok(&format!("Installed {n} skill(s).")),
        Outcome::Skipped => print_ok("Skills already up to date."),
        Outcome::Failed(msg) => {
            print_note(&format!(
                "warning: skills download failed ({msg}); continuing"
            ));
        }
    }
    Ok(())
}

/// Stage 7: create GitHub label vocabulary.
pub(super) fn labels_stage(repo: &Path, ws: &Workspace, state: &mut InitState) -> Result<()> {
    if state.is_done(Stage::Labels) {
        print_note("Resuming: labels already done — skipping.");
        return Ok(());
    }

    let triage_doc = std::fs::read_to_string(repo.join("docs/agents/triage-labels.md")).ok();
    let desired = github::ralphy_label_specs(triage_doc.as_deref());
    let existing = github::list_repo_labels(repo)?;
    let actions = github::plan_label_actions(&desired, &existing);
    print_section(
        "GitHub labels",
        Some("Labels Ralphy uses to track and triage issues:"),
    );
    for l in github::format_label_plan(&actions).lines() {
        print_bullet(l.trim());
    }

    let answer = ask_yes_no("Create/update these labels on GitHub?", true)?;
    if labels_decision(&answer) {
        with_spinner("Applying labels on GitHub…", || {
            github::apply_label_actions(&actions, repo)
        })?;
        print_ok("Labels created/updated.");
    } else {
        print_note("Skipped — labels unchanged.");
    }

    state.mark(Stage::Labels);
    state.save(ws)
}

/// Stage 8: backlog/milestone → issues (preview, confirm, publish).
pub(super) fn issues_stage(
    repo: &Path,
    ws: &Workspace,
    state: &mut InitState,
    cfg: &InitConfig,
    agent: Agent,
    model: Option<&str>,
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

    match decide_issues_path(cfg) {
        IssuesPath::Skip => {
            print_section(
                "First tasks",
                Some("No backlog or planning docs found — skipping task creation."),
            );
            // Nothing to publish — the stage completed; record it so a re-run
            // doesn't reconsider an empty backlog.
            state.mark(Stage::Issues);
            state.save(ws)
        }

        path => draft_and_publish(repo, ws, state, cfg, path, agent, model),
    }
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
    cfg: &InitConfig,
    path: IssuesPath,
    agent: Agent,
    model: Option<&str>,
) -> Result<()> {
    let (mode, source_docs) = match path {
        IssuesPath::Milestone => (IssuesMode::Milestone, cfg.milestone_docs.clone()),
        IssuesPath::LooseBacklog => (
            IssuesMode::LooseBacklog,
            cfg.backlog_location.iter().cloned().collect(),
        ),
        IssuesPath::Skip => unreachable!("Skip handled above"),
    };
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
        draft_with_agent(
            agent,
            repo,
            &draft_path,
            &req,
            model,
            Some("medium"),
            Duration::from_secs(600),
        )
    })?;
    print_note(&format!("Draft written to {}", draft_path.display()));

    println!();
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
