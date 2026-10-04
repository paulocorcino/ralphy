//! One-shot headless `claude -p` sessions that are not the plan/execute loop:
//! knowledge consolidation (`ralphy consolidate`), repo diagnosis (ADR-0012
//! stage 2), backlog → issues drafting (stage 8), and agent triage (ADR-0017).
//! Each writes its deliverable to disk and this module validates it; none ever
//! publishes to GitHub — that stays the cli's job after the operator confirms.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_adapter_support::{run_json_session, run_text_session, JsonSession, TextSession};
use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, DiagnosisReport,
    DraftRequest, IssuesDraft, TriageDraft, TriageRequest, Usage, Workspace,
};
use tracing::info;

use ralphy_core::PROMPT_CONSOLIDATE;

use crate::auth::{is_claude_auth_error, CLAUDE_AUTH_ERROR_MSG};
use crate::interactive::resolve_claude_binary;
use crate::settings::write_task_settings;
use crate::usage::parse_plan_usage;

/// Run a one-shot headless `claude -p` knowledge-consolidation session in
/// `ws`: pipe the consolidation charter on stdin and wait up to `timeout`.
/// Mirrors the planning pass's invocation (settings with the skip flags, no
/// Stop hook) — the session's only deliverable is `KNOWLEDGE.md`, which the
/// caller verifies; the consumed notes are archived by the caller, not here.
///
/// The consolidation session's tokens are captured the same way `plan` is —
/// `--output-format stream-json --verbose` makes the stdout stream carry the
/// terminal `result` event, which [`parse_plan_usage`] reads — so the run-level
/// `consolidate` ledger line carries real usage (ADR-0008 D9, issue #276).
pub fn consolidate_knowledge(
    ws: &Workspace,
    run_dir: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<Usage> {
    let settings_path = write_task_settings(run_dir)?;

    info!(?model, ?effort, "consolidating knowledge with claude -p");
    // Mirror the plan pass so the stdout stream carries the `result` event
    // `parse_plan_usage` reads; `stream-json` requires `--verbose`. `KNOWLEDGE.md`
    // is still written by the session, so the stdout format is free to change.
    let args = task_args(
        model,
        effort,
        &settings_path,
        &["--output-format", "stream-json", "--verbose"],
    );
    let cmd = task_command(&args, ws.repo_root());

    // Spawn, persist the log, bail on auth then timeout — the shared
    // `run_text_session` owns that exact tail (same messages, same order) and
    // returns the stdout stream `parse_plan_usage` reads. `KNOWLEDGE.md` is the
    // deliverable; the caller verifies it separately.
    let log = run_text_session(
        TextSession {
            cmd,
            prompt: PROMPT_CONSOLIDATE,
            timeout,
            log_path: &run_dir.join("consolidate.log"),
            spawn_err: "failed to spawn the `claude` CLI (is it installed and on PATH?)",
            auth_msg: CLAUDE_AUTH_ERROR_MSG,
            timeout_msg: "consolidation session hit the wall timeout",
        },
        is_claude_auth_error,
    )?;
    Ok(parse_plan_usage(&log))
}

/// Run a one-shot headless `claude -p` repo-diagnosis session (ADR-0012 stage 2)
/// from `neutral_cwd` — a directory OUTSIDE the target repo, so the agent CLI
/// never auto-loads the target's `CLAUDE.md`/`AGENTS.md` as system instructions.
/// The target `repo` is passed as data in the prompt; the session writes its JSON
/// report to `<neutral_cwd>/diagnosis.json`, which this function reads, validates
/// against [`DiagnosisReport`], and returns. Mirrors [`consolidate_knowledge`]'s
/// settings/auth/timeout handling.
pub fn diagnose_repo(
    repo: &Path,
    neutral_cwd: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<DiagnosisReport> {
    let settings_path = write_task_settings(neutral_cwd)?;

    let out_path = neutral_cwd.join("diagnosis.json");
    clear_stale_output(&out_path)?;

    info!(?model, ?effort, "diagnosing repo with claude -p");
    let args = task_args(model, effort, &settings_path, &[]);
    let cmd = task_command(&args, neutral_cwd);

    let log_path = neutral_cwd.join("diagnose.log");
    run_json_session(
        JsonSession {
            cmd,
            prompt: &build_diagnose_prompt(repo, &out_path),
            timeout,
            log_path: &log_path,
            out_path: &out_path,
            spawn_err: "failed to spawn the `claude` CLI (is it installed and on PATH?)",
            auth_msg: CLAUDE_AUTH_ERROR_MSG,
            timeout_msg: "diagnosis session hit the wall timeout",
            missing_msg: "diagnosis session left no report",
        },
        is_claude_auth_error,
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

/// Run a one-shot headless `claude -p` backlog/milestone → issues session
/// (ADR-0012 stage 8). Unlike [`diagnose_repo`] this runs IN the repo cwd — it
/// needs the repo's domain glossary/ADRs and (on the milestone path) writes a PRD
/// under `docs/prd/`. The session writes its [`IssuesDraft`] JSON to
/// `out_path`, which this function reads, validates against the schema, and
/// returns. It NEVER publishes to GitHub — that is the cli's job after the dev
/// confirms. Mirrors [`diagnose_repo`]'s settings/auth/timeout handling.
pub fn draft_issues(
    repo: &Path,
    out_path: &Path,
    req: &DraftRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<IssuesDraft> {
    let mode = req.mode;
    clear_stale_output(out_path)?;
    let settings_path = write_task_settings(&repo.join(".ralphy"))?;

    info!(
        ?model,
        ?effort,
        mode = mode.as_str(),
        "drafting issues with claude -p"
    );
    let args = task_args(model, effort, &settings_path, &[]);
    let cmd = task_command(&args, repo);

    let prompt = build_init_issues_prompt(repo, mode, req.source_docs, req.triage_label, out_path);
    let log_path = repo.join(".ralphy").join("init-issues.log");
    run_json_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: "failed to spawn the `claude` CLI (is it installed and on PATH?)",
            auth_msg: CLAUDE_AUTH_ERROR_MSG,
            timeout_msg: "backlog → issues session hit the wall timeout",
            missing_msg: "issues session left no draft",
        },
        is_claude_auth_error,
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

/// Run a one-shot headless `claude -p` agent-triage session (ADR-0017). Like
/// [`draft_issues`] it runs IN the repo cwd (the triage judgment reads the repo's
/// glossary/ADRs to decide whether a spec is executable) and reads each
/// `triage-agent` issue's body + full comment thread via `gh`. The session writes
/// its [`TriageDraft`] JSON to `out_path`, which this function reads and validates.
/// It NEVER publishes to GitHub — the cli applies the verdicts after the operator
/// confirms. Mirrors [`draft_issues`]'s settings/auth/timeout handling.
pub fn triage_issues(
    repo: &Path,
    out_path: &Path,
    req: &TriageRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<TriageDraft> {
    clear_stale_output(out_path)?;
    let settings_path = write_task_settings(&repo.join(".ralphy"))?;

    info!(?model, ?effort, "triaging issues with claude -p");
    let args = task_args(model, effort, &settings_path, &[]);
    let cmd = task_command(&args, repo);

    let prompt = format!(
        "{}{}{}",
        build_triage_prompt(repo, req.issue_numbers, req.queue_label, out_path),
        req.thread_block,
        req.attachments_manifest
    );
    let log_path = repo.join(".ralphy").join("triage.log");
    run_json_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: "failed to spawn the `claude` CLI (is it installed and on PATH?)",
            auth_msg: CLAUDE_AUTH_ERROR_MSG,
            timeout_msg: "triage session hit the wall timeout",
            missing_msg: "triage session left no draft",
        },
        is_claude_auth_error,
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

/// The argv of a one-shot `claude -p` task. `extra` goes between the skip flag
/// and `--settings`, the place the plan pass puts its output flags.
fn task_args(
    model: Option<&str>,
    effort: Option<&str>,
    settings_path: &Path,
    extra: &[&str],
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let Some(m) = model {
        args.push("--model".into());
        args.push(m.into());
    }
    args.push("-p".into());
    args.push("--dangerously-skip-permissions".into());
    args.extend(extra.iter().map(|flag| flag.to_string()));
    args.push("--settings".into());
    args.push(settings_path.to_string_lossy().into_owned());
    if let Some(e) = effort {
        args.push("--effort".into());
        args.push(e.into());
    }
    args
}

/// The `claude` command for a task: `args`, run in `cwd`, all three streams
/// piped.
fn task_command(args: &[String], cwd: &Path) -> Command {
    let mut cmd = Command::new(resolve_claude_binary());
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// Make the folder of `out_path` and remove a file left there by an earlier
/// run: a stale deliverable must never pass for this session's output.
fn clear_stale_output(out_path: &Path) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    match std::fs::remove_file(out_path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing the stale output {}", out_path.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_task_argv_keeps_its_flag_order() {
        let settings = Path::new("s.json");
        assert_eq!(
            task_args(None, None, settings, &[]),
            [
                "-p",
                "--dangerously-skip-permissions",
                "--settings",
                "s.json"
            ]
        );
        assert_eq!(
            task_args(Some("opus"), Some("high"), settings, &["--verbose"]),
            [
                "--model",
                "opus",
                "-p",
                "--dangerously-skip-permissions",
                "--verbose",
                "--settings",
                "s.json",
                "--effort",
                "high"
            ]
        );
    }

    #[test]
    fn a_stale_output_is_removed_and_a_missing_one_is_not_an_error() {
        let dir = tempfile::tempdir().expect("a temp dir is writable");
        let out = dir.path().join("nested").join("draft.json");
        clear_stale_output(&out).expect("a missing output is not an error");
        assert!(out.parent().is_some_and(Path::is_dir));
        std::fs::write(&out, "old").expect("the folder exists");
        clear_stale_output(&out).expect("an old output is removed");
        assert!(!out.exists());
    }
}
