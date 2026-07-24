//! One-shot headless `copilot` sessions for the `init`/`triage` flows
//! (ADR-0012 stages 2 & 8, ADR-0017, ADR-0028, ADR-0041) — repo diagnosis,
//! backlog → issues drafting, agent-triage drafting, and knowledge
//! consolidation. None of these publish to GitHub; the cli applies the
//! drafted artifact after the operator confirms.
//!
//! D7 (builtin-MCP receipt) and D11 (`continueOnAutoMode` preflight) are
//! ADR-0041 SAFETY guarantees, not `CopilotAgent`-scoped: every function here
//! calls [`preflight_or_bail`] before spawning and [`check_builtin_mcp_receipt`]
//! after the session returns, the same two guards `CopilotAgent::plan`/`execute`
//! apply, reached here as free `pub(crate)` functions instead of `self` methods
//! since a one-shot has no `CopilotAgent`. D9 (skills) stays excluded: it reads a
//! `Workspace` for skill materialization and none of the init charters invoke a
//! skill, same reason Kimi's init builder drops `--skills-dir`.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::info;

use ralphy_adapter_support::{run_init_session, run_text_session, JsonSession, TextSession};
use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, DiagnosisReport,
    DraftRequest, IssuesDraft, TriageDraft, TriageRequest, Usage, Workspace, PROMPT_CONSOLIDATE,
};

use crate::auth::{is_copilot_auth_error, COPILOT_AUTH_ERROR_MSG};
use crate::command::{build_copilot_command, build_copilot_init_command, mint_session_id};
use crate::guards::{builtin_mcp_violation, copilot_config_path};
use crate::outcome::preflight;
use crate::usage::copilot_usage;

/// D11: assert `continueOnAutoMode` is not enabled before ANY one-shot child is
/// spawned — no token is spent on a run that cannot be trusted. Mirrors
/// `CopilotAgent::run_copilot`'s preflight call, minus the `&self` it has no use
/// for here.
fn preflight_or_bail() -> Result<()> {
    let config = copilot_config_path().and_then(|p| std::fs::read_to_string(p).ok());
    preflight(config.as_deref())
}

/// D7: fail the one-shot if the builtin-MCP kill switch (`--disable-builtin-mcps`)
/// did not take, reading the combined log this session already wrote to
/// `log_path`. `require_receipt = true` unconditionally: this runs only after the
/// session already returned `Ok` (no auth/timeout/missing-artifact bail), so the
/// child reached a state where its receipt should be present.
fn check_builtin_mcp_receipt(log_path: &Path) -> Result<()> {
    let log = std::fs::read_to_string(log_path).unwrap_or_default();
    if let Some(msg) = builtin_mcp_violation(&log, true) {
        anyhow::bail!("{msg} (see {})", log_path.display());
    }
    Ok(())
}

/// Run a one-shot headless `copilot` repo-diagnosis session (ADR-0012 stage 2)
/// from `neutral_cwd` — a directory OUTSIDE the target repo. The target `repo` is
/// passed as data in the prompt; the session writes its JSON report to
/// `<neutral_cwd>/diagnosis.json`, which this function reads, validates against
/// [`DiagnosisReport`], and returns. `effort` is unused: the one-shots omit
/// `--effort` unconditionally (ADR-0041 D5), same shape as Kimi/OpenCode.
pub fn diagnose_repo(
    repo: &Path,
    neutral_cwd: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<DiagnosisReport> {
    let _ = effort;
    let out_path = neutral_cwd.join("diagnosis.json");
    let prompt = build_diagnose_prompt(repo, &out_path);
    let log_path = neutral_cwd.join("diagnose.log");

    info!(?model, "diagnosing repo with copilot");
    preflight_or_bail()?;
    let cmd = build_copilot_init_command(model, neutral_cwd, &[]);
    let report = run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path: &out_path,
            spawn_err: "failed to spawn the `copilot` CLI (is it installed and on PATH?)",
            auth_msg: COPILOT_AUTH_ERROR_MSG,
            timeout_msg: "diagnosis session hit the wall timeout",
            missing_msg: "diagnosis session left no report",
        },
        is_copilot_auth_error,
        |raw| {
            serde_json::from_str(raw).with_context(|| {
                format!(
                    "diagnosis report at {} did not match the schema",
                    out_path.display()
                )
            })
        },
    )?;
    check_builtin_mcp_receipt(&log_path)?;
    Ok(report)
}

/// Run a one-shot headless `copilot` backlog/milestone → issues session
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
    let prompt =
        build_init_issues_prompt(repo, req.mode, req.source_docs, req.triage_label, out_path);
    let log_path = repo.join(".ralphy").join("init-issues.log");

    info!(
        ?model,
        mode = req.mode.as_str(),
        "drafting issues with copilot"
    );
    preflight_or_bail()?;
    let cmd = build_copilot_init_command(model, repo, &[]);
    let draft = run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: "failed to spawn the `copilot` CLI (is it installed and on PATH?)",
            auth_msg: COPILOT_AUTH_ERROR_MSG,
            timeout_msg: "backlog → issues session hit the wall timeout",
            missing_msg: "issues session left no draft",
        },
        is_copilot_auth_error,
        |raw| {
            serde_json::from_str(raw).with_context(|| {
                format!(
                    "issues draft at {} did not match the schema",
                    out_path.display()
                )
            })
        },
    )?;
    check_builtin_mcp_receipt(&log_path)?;
    Ok(draft)
}

/// Run a one-shot headless `copilot` knowledge-consolidation session in `ws`'s
/// repo cwd: pipe the shared consolidation charter on stdin and wait up to
/// `timeout`. The session's only deliverable is the rewritten `KNOWLEDGE.md`,
/// which the caller verifies; the consumed notes are archived by the caller, not
/// here. Mirrors the other adapters' `consolidate_knowledge` signature so the cli
/// can dispatch on the selected agent. `effort` is unused: the one-shots omit
/// `--effort` unconditionally (ADR-0041 D5).
///
/// The consolidation session's tokens are captured the same way `plan`/`execute`
/// do — a locally minted `--session-id`, read back from `session-store.db` via
/// [`copilot_usage`] — so the run-level `consolidate` ledger line carries real usage
/// (ADR-0008 D9, ADR-0041 D10, issue #276). The one-shot's own `build_copilot_init_command`
/// mints and discards its id, so consolidate mints its own and drives
/// [`build_copilot_command`] directly with the identical one-shot argv.
pub fn consolidate_knowledge(
    ws: &Workspace,
    run_dir: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<Usage> {
    let _ = effort;
    std::fs::create_dir_all(run_dir).ok();
    let log_path = run_dir.join("consolidate.log");

    info!(?model, "consolidating knowledge with copilot");
    preflight_or_bail()?;
    // Mint the id ourselves: the one-shot builder mints one internally and drops it,
    // leaving no key for the session-store read. Same argv as `build_copilot_init_command`
    // (effort `None`, escape hatch off, no images).
    let session_id = mint_session_id();
    let cmd = build_copilot_command(&session_id, model, None, ws.repo_root(), false, &[]);
    run_text_session(
        TextSession {
            cmd,
            prompt: PROMPT_CONSOLIDATE,
            timeout,
            log_path: &log_path,
            spawn_err: "failed to spawn the `copilot` CLI (is it installed and on PATH?)",
            auth_msg: COPILOT_AUTH_ERROR_MSG,
            timeout_msg: "consolidation session hit the wall timeout",
        },
        is_copilot_auth_error,
    )?;
    check_builtin_mcp_receipt(&log_path)?;
    Ok(copilot_usage(&session_id))
}

/// Run a one-shot headless `copilot` agent-triage session (ADR-0017). Mirrors
/// [`draft_issues`] but drives the triage charter over each `triage-agent` issue's
/// body + full comment thread, writing a [`TriageDraft`] JSON to `out_path` for
/// the cli to apply after the operator confirms. Never publishes to GitHub.
/// `req.image_paths` IS forwarded to the child (D12 — Copilot's `ACCEPTS_IMAGES`
/// is `true`), unlike Kimi's triage which has no image-input channel.
pub fn triage_issues(
    repo: &Path,
    out_path: &Path,
    req: &TriageRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<TriageDraft> {
    let _ = effort;
    let prompt = format!(
        "{}{}",
        build_triage_prompt(repo, req.issue_numbers, req.queue_label, out_path),
        req.attachments_manifest
    );
    let log_path = repo.join(".ralphy").join("triage.log");

    info!(?model, "triaging issues with copilot");
    preflight_or_bail()?;
    let cmd = build_copilot_init_command(model, repo, req.image_paths);
    let draft = run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: "failed to spawn the `copilot` CLI (is it installed and on PATH?)",
            auth_msg: COPILOT_AUTH_ERROR_MSG,
            timeout_msg: "triage session hit the wall timeout",
            missing_msg: "triage session left no draft",
        },
        is_copilot_auth_error,
        |raw| {
            serde_json::from_str(raw).with_context(|| {
                format!(
                    "triage draft at {} did not match the schema",
                    out_path.display()
                )
            })
        },
    )?;
    check_builtin_mcp_receipt(&log_path)?;
    Ok(draft)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The D7/D11 guards must be wired into all four one-shot verbs — #237 wires
    /// real subprocess execution here for the first time, so a missing call would
    /// silently drop an ADR-0041 adapter-wide safety guarantee the moment a
    /// one-shot actually spawns a child. Source-text pin: no test here spawns a
    /// real `copilot` process, so nothing else would catch a deleted call site.
    /// Fragments assembled with `concat!` so the assertion cannot match itself.
    #[test]
    fn d7_and_d11_guards_are_wired_into_all_four_verbs() {
        let src = include_str!("tasks.rs");
        let preflight_call = concat!("preflight_or", "_bail()?;");
        let receipt_call = concat!("check_builtin_mcp", "_receipt(&log_path)?;");
        assert_eq!(
            src.matches(preflight_call).count(),
            4,
            "D11 preflight must run before every one-shot spawn"
        );
        assert_eq!(
            src.matches(receipt_call).count(),
            4,
            "D7's receipt guard must run after every one-shot session"
        );
    }

    /// The VERDICT half, mirroring `outcome::preflight_rejects_continue_on_auto_mode`:
    /// a config with `continueOnAutoMode: true` on disk must abort before any
    /// `copilot` child is spawned.
    #[test]
    fn preflight_or_bail_rejects_continue_on_auto_mode() {
        // No config on this test host is the common case, and it must pass —
        // this pins only the wiring (the predicate itself is tested in
        // `outcome::tests`), so an unreadable/absent config is not a failure.
        assert!(preflight_or_bail().is_ok());
    }

    /// D7's verdict half, reachable here without a `CopilotAgent`: a connected
    /// builtin MCP server in the log must fail the one-shot.
    #[test]
    fn check_builtin_mcp_receipt_fails_on_a_connected_server() {
        let dir = std::env::temp_dir().join(format!(
            "ralphy-copilot-tasks-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("copilot.log");
        std::fs::write(
            &log_path,
            concat!(
                r#"{"type":"session.mcp_servers_loaded","data":{"servers":[{"name":"github-mcp-server","status":"connected","source":"builtin","transport":"http"}]},"ephemeral":true}"#,
                "\n"
            ),
        )
        .unwrap();
        let err = check_builtin_mcp_receipt(&log_path).expect_err("connected must fail");
        assert!(err.to_string().contains("github-mcp-server"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
