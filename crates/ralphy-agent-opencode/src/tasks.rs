//! One-shot headless `opencode run` sessions for the `init`/`triage` flows
//! (ADR-0012 stages 2 & 8, ADR-0017) — repo diagnosis, backlog → issues
//! drafting, agent-triage drafting — and the `opencode models` passthrough.
//! None of these publish to GitHub; the cli applies the drafted artifact after
//! the operator confirms.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tracing::info;

use ralphy_adapter_support::{resolve_program, run_init_session, JsonSession};
use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, DiagnosisReport,
    DraftRequest, IssuesDraft, TriageDraft, TriageRequest,
};

use crate::command::build_opencode_command;
use crate::events::{is_opencode_auth_error, OPENCODE_AUTH_ERROR_MSG};

/// The minimal `OPENCODE_CONFIG_CONTENT` for a one-shot `init` session: an empty
/// JSON object. The diagnosis/draft sessions read the repo and write a JSON
/// artifact with the agent's own tools — they need no ralphy skills wired in, so
/// no `skills.paths` is injected (unlike `plan`/`execute`).
const INIT_OPENCODE_CONFIG: &str = "{}";

/// Run a one-shot headless `opencode run` repo-diagnosis session (ADR-0012 stage
/// 2) from `neutral_cwd` — a directory OUTSIDE the target repo, so OpenCode never
/// auto-loads the target's `AGENTS.md` as instructions. The target `repo` is
/// passed as data in the prompt; the session writes its JSON report to
/// `<neutral_cwd>/diagnosis.json`, which this function reads, validates against
/// [`DiagnosisReport`], and returns. Mirrors the Claude/Codex adapters'
/// `diagnose_repo` signature so the cli can dispatch on the selected agent.
pub fn diagnose_repo(
    repo: &Path,
    neutral_cwd: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<DiagnosisReport> {
    // OpenCode has no reasoning-effort knob (ADR-0005 D3); the parameter is
    // accepted for a uniform init dispatch signature and ignored.
    let _ = effort;
    let out_path = neutral_cwd.join("diagnosis.json");
    let prompt = build_diagnose_prompt(repo, &out_path);
    info!(?model, "diagnosing repo with opencode run");
    let cmd = build_opencode_command(model, None, neutral_cwd, INIT_OPENCODE_CONFIG);
    let log_path = neutral_cwd.join("diagnose.log");
    run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path: &out_path,
            spawn_err: "failed to spawn the `opencode` CLI (is it installed and on PATH?)",
            auth_msg: OPENCODE_AUTH_ERROR_MSG,
            timeout_msg: "diagnosis session hit the wall timeout",
            missing_msg: "diagnosis session left no report",
        },
        is_opencode_auth_error,
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

/// Run a one-shot headless `opencode run` backlog/milestone → issues session
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
    // OpenCode has no reasoning-effort knob (ADR-0005 D3); the parameter is
    // accepted for a uniform init dispatch signature and ignored.
    let _ = effort;
    let prompt =
        build_init_issues_prompt(repo, req.mode, req.source_docs, req.triage_label, out_path);
    info!(
        ?model,
        mode = req.mode.as_str(),
        "drafting issues with opencode run"
    );
    let cmd = build_opencode_command(model, None, repo, INIT_OPENCODE_CONFIG);
    let log_path = repo.join(".ralphy").join("init-issues.log");
    run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: "failed to spawn the `opencode` CLI (is it installed and on PATH?)",
            auth_msg: OPENCODE_AUTH_ERROR_MSG,
            timeout_msg: "backlog → issues session hit the wall timeout",
            missing_msg: "issues session left no draft",
        },
        is_opencode_auth_error,
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

/// Run a one-shot headless `opencode run` agent-triage session (ADR-0017).
/// Mirrors [`draft_issues`] but drives the triage charter over each `triage-agent`
/// issue's body + full comment thread, writing a [`TriageDraft`] JSON to
/// `out_path` for the cli to apply after the operator confirms. Never publishes.
pub fn triage_issues(
    repo: &Path,
    out_path: &Path,
    req: &TriageRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<TriageDraft> {
    // OpenCode has no reasoning-effort knob (ADR-0005 D3); accepted for a uniform
    // dispatch signature and ignored.
    let _ = effort;
    let prompt = build_triage_prompt(repo, req.issue_numbers, req.queue_label, out_path);
    info!(?model, "triaging issues with opencode run");
    let cmd = build_opencode_command(model, None, repo, INIT_OPENCODE_CONFIG);
    let log_path = repo.join(".ralphy").join("triage.log");
    run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: "failed to spawn the `opencode` CLI (is it installed and on PATH?)",
            auth_msg: OPENCODE_AUTH_ERROR_MSG,
            timeout_msg: "triage session hit the wall timeout",
            missing_msg: "triage session left no draft",
        },
        is_opencode_auth_error,
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

/// List available models by passing through to `opencode models`.
/// Stdio is inherited so the output streams directly to the operator.
pub fn list_models() -> Result<()> {
    let status = Command::new(resolve_program("opencode"))
        .arg("models")
        .status()
        .context("failed to spawn `opencode models` (is opencode installed and on PATH?)")?;
    if !status.success() {
        bail!("`opencode models` exited with {status}");
    }
    Ok(())
}
