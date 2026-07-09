//! One-shot headless `kimi --print` sessions for the `init`/`triage` flows
//! (ADR-0012 stages 2 & 8, ADR-0017, ADR-0028) — repo diagnosis, backlog → issues
//! drafting, and agent-triage drafting. None of these publish to GitHub; the cli
//! applies the drafted artifact after the operator confirms.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::info;

use ralphy_adapter_support::{run_init_session, run_text_session, JsonSession, TextSession};
use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, DiagnosisReport,
    DraftRequest, IssuesDraft, TriageDraft, TriageRequest, Workspace, PROMPT_CONSOLIDATE,
};

use crate::auth::{is_kimi_auth_error, KIMI_AUTH_ERROR_MSG};
use crate::command::{build_kimi_init_command, resolve_init_kimi_model};

/// Run a one-shot headless `kimi --print` repo-diagnosis session (ADR-0012 stage 2)
/// from `neutral_cwd` — a directory OUTSIDE the target repo. The target `repo` is
/// passed as data in the prompt; the session writes its JSON report to
/// `<neutral_cwd>/diagnosis.json`, which this function reads, validates against
/// [`DiagnosisReport`], and returns. `effort` is unused: Kimi has no
/// `model_reasoning_effort` analog (ADR-0028 D3), same shape as OpenCode.
pub fn diagnose_repo(
    repo: &Path,
    neutral_cwd: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<DiagnosisReport> {
    let _ = effort;
    let out_path = neutral_cwd.join("diagnosis.json");
    let model = resolve_init_kimi_model(model);
    let prompt = build_diagnose_prompt(repo, &out_path);

    info!(%model, "diagnosing repo with kimi --print");
    let cmd = build_kimi_init_command(&model, neutral_cwd);
    let log_path = neutral_cwd.join("diagnose.log");
    run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path: &out_path,
            spawn_err: "failed to spawn the `kimi` CLI (is it installed and on PATH?)",
            auth_msg: KIMI_AUTH_ERROR_MSG,
            timeout_msg: "diagnosis session hit the wall timeout",
            missing_msg: "diagnosis session left no report",
        },
        is_kimi_auth_error,
        |raw| {
            serde_json::from_str(raw).with_context(|| {
                format!(
                    "diagnosis report at {} did not match the schema",
                    out_path.display()
                )
            })
        },
    )
}

/// Run a one-shot headless `kimi --print` backlog/milestone → issues session
/// (ADR-0012 stage 8). Unlike [`diagnose_repo`] this runs IN the repo cwd — it
/// needs the repo's domain glossary/ADRs and (on the milestone path) writes a PRD
/// under `docs/prd/`. The session writes its [`IssuesDraft`] JSON to `out_path`,
/// which this function reads, validates against the schema, and returns. It NEVER
/// publishes to GitHub — that is the cli's job after the dev confirms.
pub fn draft_issues(
    repo: &Path,
    out_path: &Path,
    req: &DraftRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<IssuesDraft> {
    let _ = effort;
    let model = resolve_init_kimi_model(model);
    let prompt =
        build_init_issues_prompt(repo, req.mode, req.source_docs, req.triage_label, out_path);

    info!(%model, mode = req.mode.as_str(), "drafting issues with kimi --print");
    let cmd = build_kimi_init_command(&model, repo);
    let log_path = repo.join(".ralphy").join("init-issues.log");
    run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: "failed to spawn the `kimi` CLI (is it installed and on PATH?)",
            auth_msg: KIMI_AUTH_ERROR_MSG,
            timeout_msg: "backlog → issues session hit the wall timeout",
            missing_msg: "issues session left no draft",
        },
        is_kimi_auth_error,
        |raw| {
            serde_json::from_str(raw).with_context(|| {
                format!(
                    "issues draft at {} did not match the schema",
                    out_path.display()
                )
            })
        },
    )
}

/// Run a one-shot headless `kimi --print` knowledge-consolidation session in
/// `ws`'s repo cwd: pipe the shared consolidation charter on stdin and wait up to
/// `timeout`. The session's only deliverable is the rewritten `KNOWLEDGE.md`,
/// which the caller verifies; the consumed notes are archived by the caller, not
/// here. Mirrors the Claude adapter's `consolidate_knowledge` signature so the cli
/// can dispatch on the selected agent. `effort` is unused: Kimi has no
/// `model_reasoning_effort` analog (ADR-0028 D3), same shape as OpenCode.
pub fn consolidate_knowledge(
    ws: &Workspace,
    run_dir: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<()> {
    let _ = effort;
    std::fs::create_dir_all(run_dir).ok();
    let model = resolve_init_kimi_model(model);

    info!(%model, "consolidating knowledge with kimi --print");
    let cmd = build_kimi_init_command(&model, ws.repo_root());
    run_text_session(
        TextSession {
            cmd,
            prompt: PROMPT_CONSOLIDATE,
            timeout,
            log_path: &run_dir.join("consolidate.log"),
            spawn_err: "failed to spawn the `kimi` CLI (is it installed and on PATH?)",
            auth_msg: KIMI_AUTH_ERROR_MSG,
            timeout_msg: "consolidation session hit the wall timeout",
        },
        is_kimi_auth_error,
    )?;
    Ok(())
}

/// Run a one-shot headless `kimi --print` agent-triage session (ADR-0017). Mirrors
/// [`draft_issues`] but drives the triage charter over each `triage-agent` issue's
/// body + full comment thread, writing a [`TriageDraft`] JSON to `out_path` for
/// the cli to apply after the operator confirms. Never publishes to GitHub.
/// `req.image_paths` is unused: Kimi has no image-input channel in this slice
/// (`ACCEPTS_IMAGES = false`).
pub fn triage_issues(
    repo: &Path,
    out_path: &Path,
    req: &TriageRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<TriageDraft> {
    let _ = effort;
    let model = resolve_init_kimi_model(model);
    let prompt = format!(
        "{}{}",
        build_triage_prompt(repo, req.issue_numbers, req.queue_label, out_path),
        req.attachments_manifest
    );

    info!(%model, "triaging issues with kimi --print");
    let cmd = build_kimi_init_command(&model, repo);
    let log_path = repo.join(".ralphy").join("triage.log");
    run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: "failed to spawn the `kimi` CLI (is it installed and on PATH?)",
            auth_msg: KIMI_AUTH_ERROR_MSG,
            timeout_msg: "triage session hit the wall timeout",
            missing_msg: "triage session left no draft",
        },
        is_kimi_auth_error,
        |raw| {
            serde_json::from_str(raw).with_context(|| {
                format!(
                    "triage draft at {} did not match the schema",
                    out_path.display()
                )
            })
        },
    )
}
