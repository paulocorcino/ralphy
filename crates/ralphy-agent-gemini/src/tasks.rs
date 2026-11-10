//! One-shot headless `gemini` sessions for the `init`/`triage` flows (ADR-0012
//! stages 2 & 8, ADR-0017, ADR-0043) — repo diagnosis, backlog → issues drafting,
//! agent-triage drafting, and knowledge consolidation. None of these publish to
//! GitHub; the cli applies the drafted artifact after the operator confirms.
//!
//! **The isolation is not a run-loop property, it is an adapter one.** Every verb
//! here pays the SAME [`crate::prepare_root`] a run pays — the owned
//! configuration root (D4), the sovereign policy document (D5), the
//! administrator-tier bail (D5) — through the one command builder
//! [`crate::command::build_gemini_command`], never a one-shot-shaped copy of it.
//! A second builder is precisely the drift that lets a one-shot read the
//! operator's own `~/.gemini`.
//!
//! The root's BASE is per-workspace (`<repo>/.ralphy`), with a defined
//! no-workspace fallback (D4): `<home>/.ralphy`, the same base
//! `auth::probe_gemini_login` ensures, so a machine ends with one root rather than
//! two. `draft_issues` and `consolidate_knowledge` legitimately run where there is
//! no repository at all, and must reach their spawn rather than be refused.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use tracing::info;

use ralphy_adapter_support::{failed_run_stop, RunFacts, RunStop};
use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, DiagnosisReport,
    DraftRequest, IssuesDraft, TriageDraft, TriageRequest, Usage, Workspace, PROMPT_CONSOLIDATE,
};

use crate::auth::{is_gemini_auth_error, GEMINI_AUTH_ERROR_MSG};
use crate::command::{self, build_gemini_command, check_stdin_ceiling, mint_session_id};
use crate::{outcome, revocation, PreparedRoot};

/// The spawn-failure sentence every verb shares, matching the run path's.
const SPAWN_ERR: &str = "failed to spawn the `gemini` CLI (is it installed?)";

/// Where this one-shot's owned root lives (D4).
///
/// `<repo>/.ralphy` when `repo` is a repository — the same base a run uses, so a
/// diagnosis and the run that follows it share one identity. Otherwise
/// `<home>/.ralphy`, which is exactly what `auth::probe_gemini_login` ensures.
/// A home that cannot be named falls back to the system temp dir with a warning
/// rather than a bail: a one-shot the operator asked for still runs, and the cost
/// is identity persistence, which is logged.
pub(crate) fn one_shot_base(repo: &Path) -> PathBuf {
    if ralphy_core::git::is_repo(repo) {
        return repo.join(".ralphy");
    }
    if let Some(home) = ralphy_proc_util::home_dir() {
        home.join(".ralphy")
    } else {
        tracing::warn!(
            "gemini: no home directory — the one-shot's configuration root falls back to \
             the system temp dir, so its installation identity will not persist"
        );
        std::env::temp_dir().join("ralphy")
    }
}

/// Build the one-shot's child command, having first prepared the owned root and
/// written the policy document under `base` (D4/D5). The single call site of
/// [`build_gemini_command`] on this path — the argv IS the isolation here, so no
/// verb gets to choose its own root argument.
///
/// The skill-discovery receipt is deliberately NOT paid here: it is an advisory
/// extra child spawn per verb, and a one-shot acts on nothing it reports.
pub(crate) fn one_shot_command(
    base: &Path,
    work_dir: &Path,
    model: Option<&str>,
) -> Result<Command> {
    let PreparedRoot {
        root,
        policy_path,
        auth_type,
        ..
    } = crate::prepare_root(base)?;
    Ok(build_gemini_command(
        &mint_session_id(),
        model,
        work_dir,
        &root.home,
        &policy_path,
        auth_type.as_deref(),
    ))
}

/// Whether a one-shot's child SUCCEEDED: a natural exit `0` that was not killed.
///
/// This is the gate [`one_shot_stop`] must not be consulted past. Two of the
/// ladder's rungs key on free text in the combined log, and on this vendor that
/// text is routine rather than diagnostic: a managed host prints "disabled by
/// administrator" in EVERY log, and `draft_issues`/`triage_issues` pipe the
/// model's own prose through stdout, so a backlog that merely MENTIONS a rate
/// limit would otherwise be reported as one. Both run-path counterparts gate the
/// same way — `plan()`'s ladder is `run_plan_session`'s `on_missing`, consulted
/// only when no plan was written, and `classify_gemini_outcome`'s is
/// `(!succeeded).then(…)` ("a revocation must never flip a run that succeeded").
fn one_shot_succeeded(out: &ralphy_adapter_support::HeadlessOutput) -> bool {
    out.exited_cleanly()
}

/// The one-shot's stop ladder for a session that did NOT succeed, in the SAME
/// order `plan()` uses: a hard-stop revocation, then the wall timeout, then a
/// provider limit, then an informational revocation notice, then the exit-code
/// taxonomy. `None` means the failure carries no sentence better than the
/// caller's own.
///
/// The timeout sits second rather than last (where a naive reading of `plan()`'s
/// ladder would put it): on a timeout `exit` is `None`, so the exit-code rung is
/// inert, and a routine administrator notice in the log would otherwise be
/// reported instead of the reap. Both shared session runners bail on a timeout
/// immediately after auth for the same reason.
///
/// It is exit-code FIRST in the sense that matters (D3): the vendor's own exit
/// taxonomy is consulted here rather than discarded, which is why the one-shots
/// cannot go through `run_text_session` — that runner drops the child's status.
pub(crate) fn one_shot_stop(log: &str, exit_code: Option<i32>, timed_out: bool) -> Option<String> {
    let rev = revocation::detect_revocation(log);
    rev.filter(|r| r.is_hard_stop())
        .map(|r| r.message(exit_code, log))
        .or_else(|| timed_out.then(|| "gemini session hit the wall timeout".to_string()))
        .or_else(|| outcome::gemini_limit_note(log))
        .or_else(|| rev.map(|r| r.message(exit_code, log)))
        .or_else(|| {
            outcome::classify_exit(exit_code)
                .actionable_stop()
                .map(str::to_string)
        })
}

/// Drive one headless `gemini` child to completion, persist its combined log, and
/// — only when the child did NOT succeed — turn the failure into the sentence
/// [`one_shot_stop`] chose.
///
/// **Cross-path invariant:** the log is written on EVERY return path after the
/// child ran — including each bail — so a failing one-shot never leaves the
/// operator without the log its own error message points at.
///
/// The prompt's stdin ceiling (D2) is checked by each VERB before it prepares a
/// root, not here: by the time this is reached the root, the skills and the
/// policy document have already been written, and a charter Ralphy refuses to
/// send must cost none of that.
fn run_one_shot(cmd: Command, prompt: &str, timeout: Duration, log_path: &Path) -> Result<String> {
    let out = ralphy_adapter_support::run_headless(cmd, prompt, timeout).context(SPAWN_ERR)?;
    judge_one_shot(out, log_path)
}

/// Everything [`run_one_shot`] does once the child has finished: persist the log,
/// keep the log of a session that succeeded, and turn any other end into an error.
/// A login error in the log is judged only for a session that did not succeed:
/// the log of a good run may quote one.
fn judge_one_shot(out: ralphy_adapter_support::HeadlessOutput, log_path: &Path) -> Result<String> {
    let succeeded = one_shot_succeeded(&out);
    let facts = RunFacts::from(&out);
    let (code, timed_out) = (out.exit.and_then(|s| s.code()), out.timed_out);
    let mut log = out.stdout;
    log.push_str(&out.stderr);
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the log is a copy for the operator; the session result stands without it"
    )]
    let _ = fs::write(log_path, &log);

    if succeeded {
        // The `--output-format stream-json` stream is captured in `log`; callers
        // that need token usage (consolidate) fold it, the rest drop it.
        return Ok(log);
    }
    match failed_run_stop(&facts, &log, is_gemini_auth_error) {
        Some(RunStop::Auth) => bail!("{GEMINI_AUTH_ERROR_MSG} (see {})", log_path.display()),
        // The one-shot ladder below names the timeout and the stop in its own
        // order: a hard-stop revocation comes before the timeout.
        Some(RunStop::Stopped | RunStop::Timeout) | None => {}
    }
    match one_shot_stop(&log, code, timed_out) {
        Some(msg) => bail!("{msg} (see {})", log_path.display()),
        // A non-zero exit the taxonomy does not name is still a failure: saying so
        // beats returning `Ok` and letting the caller report a missing artifact.
        None => bail!(
            "the gemini session failed (exit {}) — see {}",
            code.map_or_else(|| "killed".to_string(), |c| c.to_string()),
            log_path.display()
        ),
    }
}

/// Ensure the artifact's and the log's parent dirs exist, then drop any stale
/// artifact — a prior run's file must never masquerade as this session's output.
fn clear_stale_artifact(out_path: &Path, log_path: &Path) -> Result<()> {
    for parent in [out_path.parent(), log_path.parent()].into_iter().flatten() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    match fs::remove_file(out_path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing the stale output {}", out_path.display()))
        }
        Err(_) | Ok(()) => Ok(()),
    }
}

/// Read and validate the JSON artifact a one-shot was asked to write.
fn read_artifact<T: DeserializeOwned>(
    out_path: &Path,
    log_path: &Path,
    missing_msg: &str,
    label: &str,
) -> Result<T> {
    let raw = fs::read_to_string(out_path).with_context(|| {
        format!(
            "{} at {} (see {})",
            missing_msg,
            out_path.display(),
            log_path.display()
        )
    })?;
    // A vendor CLI on Windows may write the artifact UTF-8-BOM-prefixed; left in
    // place the BOM reads as a schema mismatch at "line 1 column 1".
    serde_json::from_str(ralphy_adapter_support::strip_bom(&raw)).with_context(|| {
        format!(
            "{} at {} did not match the schema",
            label,
            out_path.display()
        )
    })
}

/// Run a one-shot headless `gemini` repo-diagnosis session (ADR-0012 stage 2)
/// from `neutral_cwd` — a directory OUTSIDE the target repo, so the CLI cannot
/// auto-load the target's own agent instructions. The target `repo` is passed as
/// data in the prompt.
///
/// The owned root's base is still the TARGET's (`one_shot_base(repo)`), not the
/// throwaway cwd's: the identity belongs to the repository being diagnosed, and a
/// root under a temp dir the caller deletes would be a new installation every run.
/// `effort` is a documented no-op (ADR-0044 D4): accepted for a uniform init
/// dispatch signature, discarded here — Gemini has no level axis on argv;
/// numeric `thinkingBudget` mapping is deliberately deferred.
pub fn diagnose_repo(
    repo: &Path,
    neutral_cwd: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<DiagnosisReport> {
    // ADR-0044 D4 No-op: neutral Effort word discarded; must not alter argv.
    let _ = effort;
    let out_path = neutral_cwd.join("diagnosis.json");
    let log_path = neutral_cwd.join("diagnose.log");
    let prompt = build_diagnose_prompt(repo, &out_path);
    // D2 before any side effect: a charter ralphy refuses to send must cost
    // neither a materialized root nor a written policy document.
    check_stdin_ceiling(&prompt)?;
    clear_stale_artifact(&out_path, &log_path)?;

    info!(?model, "diagnosing repo with gemini");
    let cmd = one_shot_command(&one_shot_base(repo), neutral_cwd, model)?;
    run_one_shot(cmd, &prompt, timeout, &log_path)?;
    read_artifact(
        &out_path,
        &log_path,
        "diagnosis session left no report",
        "diagnosis report",
    )
}

/// Run a one-shot headless `gemini` backlog/milestone → issues session (ADR-0012
/// stage 8). Unlike [`diagnose_repo`] this runs IN the repo cwd — it needs the
/// repo's domain glossary/ADRs and (on the milestone path) writes a PRD under
/// `docs/prd/`. Never publishes to GitHub: that is the cli's job after the
/// operator confirms.
pub fn draft_issues(
    repo: &Path,
    out_path: &Path,
    req: &DraftRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<IssuesDraft> {
    // ADR-0044 D4 No-op: neutral Effort word discarded; must not alter argv.
    let _ = effort;
    let prompt =
        build_init_issues_prompt(repo, req.mode, req.source_docs, req.triage_label, out_path);
    let log_path = repo.join(".ralphy").join("init-issues.log");
    check_stdin_ceiling(&prompt)?;
    clear_stale_artifact(out_path, &log_path)?;

    info!(
        ?model,
        mode = req.mode.as_str(),
        "drafting issues with gemini"
    );
    let cmd = one_shot_command(&one_shot_base(repo), repo, model)?;
    run_one_shot(cmd, &prompt, timeout, &log_path)?;
    read_artifact(
        out_path,
        &log_path,
        "issues session left no draft",
        "issues draft",
    )
}

/// Assemble the triage charter: the core prompt, then the issue threads, then
/// `attachments_manifest`
/// (the vendor-neutral textual inventory, ADR-0025 §6), then this vendor's own
/// `@`-reference block per fetched image (ADR-0043 D14). The `@` syntax is
/// this vendor's alone, so it is appended here rather than folded into the
/// core-owned manifest.
pub(crate) fn triage_prompt(repo: &Path, req: &TriageRequest, out_path: &Path) -> String {
    format!(
        "{}{}{}{}",
        build_triage_prompt(repo, req.issue_numbers, req.queue_label, out_path),
        req.thread_block,
        req.attachments_manifest,
        command::attachment_block(req.image_paths)
    )
}

/// Run a one-shot headless `gemini` agent-triage session (ADR-0017). Mirrors
/// [`draft_issues`] but drives the triage charter over each `triage-agent` issue's
/// body + full comment thread, writing a [`TriageDraft`] JSON to `out_path` for the
/// cli to apply after the operator confirms.
pub fn triage_issues(
    repo: &Path,
    out_path: &Path,
    req: &TriageRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<TriageDraft> {
    // ADR-0044 D4 No-op: neutral Effort word discarded; must not alter argv.
    let _ = effort;
    let prompt = triage_prompt(repo, req, out_path);
    let log_path = repo.join(".ralphy").join("triage.log");
    check_stdin_ceiling(&prompt)?;
    clear_stale_artifact(out_path, &log_path)?;

    info!(?model, "triaging issues with gemini");
    let mut cmd = one_shot_command(&one_shot_base(repo), repo, model)?;
    command::add_include_directories(&mut cmd, &command::attachment_dirs(req.image_paths));
    run_one_shot(cmd, &prompt, timeout, &log_path)?;
    read_artifact(
        out_path,
        &log_path,
        "triage session left no draft",
        "triage draft",
    )
}

/// Run a one-shot headless `gemini` knowledge-consolidation session in `ws`'s repo
/// cwd: pipe the shared consolidation charter on stdin and wait up to `timeout`.
/// The session's only deliverable is the rewritten `KNOWLEDGE.md`, which the caller
/// verifies; the consumed notes are archived by the caller, not here.
///
/// The consolidation session's tokens are captured the same way `plan`/`execute`
/// are — the `--output-format stream-json` stream carries the terminal `result.stats`,
/// which `crate::phase_usage` reads off `outcome::fold_gemini_stream` — so the
/// run-level `consolidate` ledger line carries real usage (ADR-0008 D9, issue #276).
pub fn consolidate_knowledge(
    ws: &Workspace,
    run_dir: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<Usage> {
    // ADR-0044 D4 No-op: neutral Effort word discarded; must not alter argv.
    let _ = effort;
    check_stdin_ceiling(PROMPT_CONSOLIDATE)?;
    fs::create_dir_all(run_dir).context("creating the run directory")?;
    let log_path = run_dir.join("consolidate.log");

    info!(?model, "consolidating knowledge with gemini");
    let cmd = one_shot_command(&one_shot_base(ws.repo_root()), ws.repo_root(), model)?;
    let log = run_one_shot(cmd, PROMPT_CONSOLIDATE, timeout, &log_path)?;
    Ok(crate::phase_usage(
        Some(&outcome::fold_gemini_stream(&log)),
        model,
    ))
}

#[cfg(test)]
mod tests;
