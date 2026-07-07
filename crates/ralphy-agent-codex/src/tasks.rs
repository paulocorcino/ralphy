//! One-shot headless `codex exec` sessions for the `init`/`triage` flows
//! (ADR-0012 stages 2 & 8, ADR-0017) — repo diagnosis, backlog → issues
//! drafting, and agent-triage drafting. None of these publish to GitHub; the
//! cli applies the drafted artifact after the operator confirms.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::info;

use ralphy_adapter_support::{run_init_session, JsonSession};
use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, DiagnosisReport,
    DraftRequest, IssuesDraft, TriageDraft, TriageRequest,
};

use crate::auth::{is_codex_auth_error, CODEX_AUTH_ERROR_MSG};
use crate::command::{build_codex_init_command, resolve_init_model};

/// Run a one-shot headless `codex exec` repo-diagnosis session (ADR-0012 stage 2)
/// from `neutral_cwd` — a directory OUTSIDE the target repo, so Codex never
/// auto-loads the target's `AGENTS.md` as instructions. The target `repo` is
/// passed as data in the prompt; the session writes its JSON report to
/// `<neutral_cwd>/diagnosis.json`, which this function reads, validates against
/// [`DiagnosisReport`], and returns. Mirrors the Claude adapter's
/// `diagnose_repo` signature so the cli can dispatch on the selected agent.
pub fn diagnose_repo(
    repo: &Path,
    neutral_cwd: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<DiagnosisReport> {
    let out_path = neutral_cwd.join("diagnosis.json");
    let model = resolve_init_model(model);
    let effort = effort.unwrap_or("medium");
    let prompt = build_diagnose_prompt(repo, &out_path);

    info!(%model, effort, "diagnosing repo with codex exec");
    let cmd = build_codex_init_command(&model, effort, neutral_cwd, &[]);
    let log_path = neutral_cwd.join("diagnose.log");
    run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path: &out_path,
            spawn_err: "failed to spawn the `codex` CLI (is it installed and on PATH?)",
            auth_msg: CODEX_AUTH_ERROR_MSG,
            timeout_msg: "diagnosis session hit the wall timeout",
            missing_msg: "diagnosis session left no report",
        },
        is_codex_auth_error,
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

/// Run a one-shot headless `codex exec` backlog/milestone → issues session
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
    let model = resolve_init_model(model);
    let effort = effort.unwrap_or("medium");
    let prompt =
        build_init_issues_prompt(repo, req.mode, req.source_docs, req.triage_label, out_path);

    info!(%model, effort, mode = req.mode.as_str(), "drafting issues with codex exec");
    let cmd = build_codex_init_command(&model, effort, repo, &[]);
    let log_path = repo.join(".ralphy").join("init-issues.log");
    run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: "failed to spawn the `codex` CLI (is it installed and on PATH?)",
            auth_msg: CODEX_AUTH_ERROR_MSG,
            timeout_msg: "backlog → issues session hit the wall timeout",
            missing_msg: "issues session left no draft",
        },
        is_codex_auth_error,
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

/// Run a one-shot headless `codex exec` agent-triage session (ADR-0017). Mirrors
/// [`draft_issues`] but drives the triage charter over each `triage-agent` issue's
/// body + full comment thread, writing a [`TriageDraft`] JSON to `out_path` for
/// the cli to apply after the operator confirms. Never publishes to GitHub.
pub fn triage_issues(
    repo: &Path,
    out_path: &Path,
    req: &TriageRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<TriageDraft> {
    let model = resolve_init_model(model);
    let effort = effort.unwrap_or("medium");
    let prompt = format!(
        "{}{}",
        build_triage_prompt(repo, req.issue_numbers, req.queue_label, out_path),
        req.attachments_manifest
    );

    info!(%model, effort, "triaging issues with codex exec");
    let cmd = build_codex_init_command(&model, effort, repo, req.image_paths);
    let log_path = repo.join(".ralphy").join("triage.log");
    run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: "failed to spawn the `codex` CLI (is it installed and on PATH?)",
            auth_msg: CODEX_AUTH_ERROR_MSG,
            timeout_msg: "triage session hit the wall timeout",
            missing_msg: "triage session left no draft",
        },
        is_codex_auth_error,
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
