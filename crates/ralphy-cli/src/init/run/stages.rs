//! The stages of `ralphy init`, one function per stage of the checkpoint
//! (`InitState`). [`super::run`] calls them in order; each stage reads and
//! writes the checkpoint itself, so a re-run resumes where the last one ended.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ralphy_core::{git, github, DiagnosisReport, Workspace};

use super::decisions::{
    branch_decision, commit_decision, labels_decision, BranchDecision, CommitDecision,
};
use super::{diagnose_with_agent, diagnosis_cwd};
use crate::init::gate::{evaluate_gate, findings_from, Agent, EnvFindings};
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
        findings_from(&crate::doctor::probe(), repo)
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
    let fetch_ref = SKILLS_REF.to_owned();
    let subtree = SKILLS_SUBTREE.to_owned();
    let fetch = |scratch: &Path| -> Result<PathBuf> {
        for argv in sparse_fetch_commands(&fetch_ref, &subtree) {
            let args: Vec<&str> = argv.iter().map(String::as_str).collect();
            git::git(scratch, &args)?;
        }

        Ok(scratch.join(&subtree))
    };
    let outcome = with_spinner("Installing skills…", || {
        install_skills_step(skills_dst, fetch)
    });
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

mod issues;
pub(super) use issues::issues_stage;
