//! The one-shot tasks: repo diagnosis, backlog to issues, agent triage and
//! knowledge consolidation. Each runs one held `agy` session through
//! [`Session`], so it gets the owned root, the proof turn and the guard as a
//! run does (ADR-0086 D8, D9). None publishes to GitHub: the CLI applies the
//! drafted file after the operator confirms.
#![expect(dead_code, reason = "used once the Agent impl lands")]

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ralphy_adapter_support::{failed_run_stop, remove_stale_file, HeadlessRun, RunFacts, RunStop};
use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, DiagnosisReport,
    DraftRequest, IssuesDraft, TriageDraft, TriageRequest, Usage, Workspace, PROMPT_CONSOLIDATE,
};
use serde::de::DeserializeOwned;

use crate::outcome::{self, RunEnd};
use crate::session::Session;

/// The settings every one-shot task takes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OneShotOptions<'a> {
    /// The `--model` id; `None` uses the default of the operator's plan.
    pub(crate) model: Option<&'a str>,
    /// The effort word, passed as `--effort` with a base model id.
    pub(crate) effort: Option<&'a str>,
    /// The wall-clock budget of the session.
    pub(crate) timeout: Duration,
    /// The operator lets a session whose hooks did not prove they run go on.
    pub(crate) run_without_guard_proof: bool,
}

/// Diagnose `repo` from `neutral_cwd`, a folder outside the repository, so
/// `agy` loads none of the repository's own agent files. The session writes
/// `diagnosis.json` there.
pub(crate) fn diagnose_repo(
    repo: &Path,
    neutral_cwd: &Path,
    options: OneShotOptions<'_>,
) -> Result<DiagnosisReport> {
    let out_path = neutral_cwd.join("diagnosis.json");
    clear_stale(&out_path)?;
    let log_path = neutral_cwd.join("diagnose.log");
    let task = build_diagnose_prompt(repo, &out_path);
    held_one_shot(neutral_cwd, &log_path, options, "diagnosis", &task)?;
    read_output(&out_path, &log_path, "diagnosis report")
}

/// Draft issues from a backlog or a milestone, in the repository, into
/// `out_path`.
pub(crate) fn draft_issues(
    repo: &Path,
    out_path: &Path,
    req: &DraftRequest<'_>,
    options: OneShotOptions<'_>,
) -> Result<IssuesDraft> {
    clear_stale(out_path)?;
    let log_path = repo.join(".ralphy").join("init-issues.log");
    let task =
        build_init_issues_prompt(repo, req.mode, req.source_docs, req.triage_label, out_path);
    held_one_shot(repo, &log_path, options, "issues", &task)?;
    read_output(out_path, &log_path, "issues draft")
}

/// Triage the issues of `req` in the repository: the charter, each issue's
/// thread, then the attachment list, whose images `agy` reads by path.
pub(crate) fn triage_issues(
    repo: &Path,
    out_path: &Path,
    req: &TriageRequest<'_>,
    options: OneShotOptions<'_>,
) -> Result<TriageDraft> {
    clear_stale(out_path)?;
    let log_path = repo.join(".ralphy").join("triage.log");
    let charter = build_triage_prompt(repo, req.issue_numbers, req.queue_label, out_path);
    let task = [charter.as_str(), req.thread_block, req.attachments_manifest].concat();
    held_one_shot(repo, &log_path, options, "triage", &task)?;
    read_output(out_path, &log_path, "triage draft")
}

/// Consolidate the knowledge notes of `ws` into `KNOWLEDGE.md`, which the
/// caller checks. The usage is the sum of the session's model calls, the
/// proof turn's included.
pub(crate) fn consolidate_knowledge(
    ws: &Workspace,
    run_dir: &Path,
    options: OneShotOptions<'_>,
) -> Result<Usage> {
    let log_path = run_dir.join("consolidate.log");
    let run = held_one_shot(
        ws.repo_root(),
        &log_path,
        options,
        "consolidation",
        PROMPT_CONSOLIDATE,
    )?;
    Ok(crate::usage::usage(
        &outcome::fold(&run.stdout),
        options.model,
    ))
}

/// Run `task` in one held session in `work_dir`, with its status file beside
/// the log, and turn a failed end into an error that names `what`.
fn held_one_shot(
    work_dir: &Path,
    log_path: &Path,
    options: OneShotOptions<'_>,
    what: &str,
    task: &str,
) -> Result<HeadlessRun> {
    let root = ralphy_proc_util::antigravity::owned_root()?;
    let session = Session {
        root: &root,
        work_dir,
        run_dir: log_path.parent().unwrap_or(work_dir),
        log_path,
        model: options.model,
        effort: options.effort,
        waiver: options.run_without_guard_proof,
        timeout: options.timeout,
        idle_minutes: 0,
    };
    let run = session.run(task)?;
    let see = log_path.display();
    match failed_run_stop(&RunFacts::from(&run), &run.log, crate::auth::is_auth_error) {
        Some(RunStop::Auth) => bail!("{} (see {see})", crate::auth::auth_message(&root)),
        Some(RunStop::Timeout) => bail!("the {what} session hit the wall timeout (see {see})"),
        Some(RunStop::Stopped) | None => {}
    }
    let stderr = run
        .log
        .strip_prefix(run.stdout.as_str())
        .unwrap_or(&run.log);
    answer(&run.stdout, stderr)
        .with_context(|| format!("the {what} session failed (see {see})"))?;
    Ok(run)
}

/// The answer of a session: the response of its LAST `result` line, never the
/// proof turn's. An error when that turn failed or the stream has no result.
pub(crate) fn answer(stdout: &str, stderr: &str) -> Result<String> {
    let fold = outcome::fold(stdout);
    let failed = outcome::completion_signals(&fold, stderr, RunEnd::default()).errored;
    let Some(result) = fold.result else {
        bail!("agy wrote no final result");
    };
    if failed {
        let why = result.error.unwrap_or(result.status);
        bail!("agy ended the task with an error: {why}");
    }
    Ok(result.response)
}

/// Make the folder of `out_path` and remove the file an earlier session left
/// there: it is not this session's output.
fn clear_stale(out_path: &Path) -> Result<()> {
    if let Some(dir) = out_path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    remove_stale_file(out_path);
    Ok(())
}

/// The JSON file the session wrote at `out_path`, read as `T`.
fn read_output<T: DeserializeOwned>(out_path: &Path, log_path: &Path, label: &str) -> Result<T> {
    let text = fs::read_to_string(out_path).with_context(|| {
        format!(
            "the session left no {label} at {} (see {})",
            out_path.display(),
            log_path.display()
        )
    })?;
    let text = ralphy_adapter_support::strip_bom(&text);
    serde_json::from_str(text).with_context(|| {
        format!(
            "the {label} at {} does not match the schema",
            out_path.display()
        )
    })
}

#[cfg(test)]
mod tests;
