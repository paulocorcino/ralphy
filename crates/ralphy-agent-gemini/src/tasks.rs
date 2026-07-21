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

use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, DiagnosisReport,
    DraftRequest, IssuesDraft, TriageDraft, TriageRequest, Workspace, PROMPT_CONSOLIDATE,
};

use crate::auth::{is_gemini_auth_error, GEMINI_AUTH_ERROR_MSG};
use crate::command::{build_gemini_command, check_stdin_ceiling, mint_session_id};
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
    match ralphy_proc_util::home_dir() {
        Some(home) => home.join(".ralphy"),
        None => {
            tracing::warn!(
                "gemini: no home directory — the one-shot's configuration root falls back to \
                 the system temp dir, so its installation identity will not persist"
            );
            std::env::temp_dir().join("ralphy")
        }
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

/// The one-shot's stop ladder, in the SAME order `plan()` uses: a hard-stop
/// revocation outranks a provider limit, which outranks an informational
/// revocation notice, which outranks the exit-code taxonomy, which outranks the
/// wall timeout. `None` means nothing actionable was found.
///
/// It is exit-code FIRST in the sense that matters (D3): the vendor's own exit
/// taxonomy is consulted here rather than discarded, which is why the one-shots
/// cannot go through `run_text_session` — that runner drops the child's status.
pub(crate) fn one_shot_stop(log: &str, exit_code: Option<i32>, timed_out: bool) -> Option<String> {
    let rev = revocation::detect_revocation(log);
    rev.filter(|r| r.is_hard_stop())
        .map(|r| r.message(exit_code, log))
        .or_else(|| outcome::gemini_limit_note(log))
        .or_else(|| rev.map(|r| r.message(exit_code, log)))
        .or_else(|| {
            outcome::classify_exit(exit_code)
                .actionable_stop()
                .map(str::to_string)
        })
        .or_else(|| timed_out.then(|| "gemini session hit the wall timeout".to_string()))
}

/// Drive one headless `gemini` child to completion, persist its combined log, and
/// turn the result into an error through [`one_shot_stop`].
///
/// **Cross-path invariant:** the log is written on EVERY return path after the
/// child ran — including each ladder bail — so a failing one-shot never leaves the
/// operator without the log its own error message points at.
fn run_one_shot(cmd: Command, prompt: &str, timeout: Duration, log_path: &Path) -> Result<()> {
    // D2 first: a truncated charter is a session running without its rules.
    check_stdin_ceiling(prompt)?;
    let out = ralphy_adapter_support::run_headless(cmd, prompt, timeout).context(SPAWN_ERR)?;
    let mut log = out.stdout;
    log.push_str(&out.stderr);
    let _ = fs::write(log_path, &log);

    if is_gemini_auth_error(&log) {
        bail!("{GEMINI_AUTH_ERROR_MSG} (see {})", log_path.display());
    }
    if let Some(msg) = one_shot_stop(&log, out.exit.and_then(|s| s.code()), out.timed_out) {
        bail!("{msg} (see {})", log_path.display());
    }
    Ok(())
}

/// Ensure the artifact's and the log's parent dirs exist, then drop any stale
/// artifact — a prior run's file must never masquerade as this session's output.
fn clear_stale_artifact(out_path: &Path, log_path: &Path) {
    for parent in [out_path.parent(), log_path.parent()].into_iter().flatten() {
        fs::create_dir_all(parent).ok();
    }
    let _ = fs::remove_file(out_path);
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
/// `effort` is unused — this vendor's headless surface has no reasoning-effort
/// axis.
pub fn diagnose_repo(
    repo: &Path,
    neutral_cwd: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<DiagnosisReport> {
    let _ = effort;
    let out_path = neutral_cwd.join("diagnosis.json");
    let log_path = neutral_cwd.join("diagnose.log");
    let prompt = build_diagnose_prompt(repo, &out_path);
    clear_stale_artifact(&out_path, &log_path);

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
    let _ = effort;
    let prompt =
        build_init_issues_prompt(repo, req.mode, req.source_docs, req.triage_label, out_path);
    let log_path = repo.join(".ralphy").join("init-issues.log");
    clear_stale_artifact(out_path, &log_path);

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

/// Run a one-shot headless `gemini` agent-triage session (ADR-0017). Mirrors
/// [`draft_issues`] but drives the triage charter over each `triage-agent` issue's
/// body + full comment thread, writing a [`TriageDraft`] JSON to `out_path` for the
/// cli to apply after the operator confirms.
///
/// `req.image_paths` is unused HERE although this vendor accepts attachments
/// (`ACCEPTS_IMAGES = true`, D14): wiring the `@<path>` interpolation is its own
/// slice. Only `attachments_manifest` — the textual inventory the core charter
/// builds — reaches the child, exactly as on every vendor.
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
    clear_stale_artifact(out_path, &log_path);

    info!(?model, "triaging issues with gemini");
    let cmd = one_shot_command(&one_shot_base(repo), repo, model)?;
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
pub fn consolidate_knowledge(
    ws: &Workspace,
    run_dir: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<()> {
    let _ = effort;
    fs::create_dir_all(run_dir).ok();
    let log_path = run_dir.join("consolidate.log");

    info!(?model, "consolidating knowledge with gemini");
    let cmd = one_shot_command(&one_shot_base(ws.repo_root()), ws.repo_root(), model)?;
    run_one_shot(cmd, PROMPT_CONSOLIDATE, timeout, &log_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The argv/env the tests read back out of a built `Command`.
    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// D4's no-workspace case, which the plain "join `.ralphy` onto the repo"
    /// implementation gets silently wrong: `draft_issues` and
    /// `consolidate_knowledge` legitimately run outside any repository, and the
    /// root they land on must be the SAME `<home>/.ralphy` the login probe
    /// ensures — asserted as that literal value, not merely "some path".
    #[test]
    fn one_shot_base_falls_back_outside_a_repository() {
        // A REAL repository: `git::is_repo` shells out to `rev-parse`, which an
        // empty `.git` directory does not satisfy.
        let repo = tempfile::tempdir().expect("tempdir");
        ralphy_core::git::init(repo.path()).expect("git init");
        assert_eq!(
            one_shot_base(repo.path()),
            repo.path().join(".ralphy"),
            "in a workspace the one-shot shares the run's root"
        );

        let plain = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            one_shot_base(plain.path()),
            ralphy_proc_util::home_dir()
                .expect("this host has a home directory")
                .join(".ralphy"),
            "with no workspace the one-shot shares the login probe's root"
        );
    }

    /// AC2, on the one-shot path: the child is pointed at the root Ralphy owns and
    /// carries Ralphy's own policy document — neither is bypassed here. No child is
    /// spawned: the discovery probe lives outside `prepare_root`, which is what
    /// keeps this test spawn-free.
    #[test]
    fn the_one_shot_command_carries_the_owned_root_and_the_policy() {
        let base = tempfile::tempdir().expect("tempdir");
        let work = tempfile::tempdir().expect("tempdir");
        let cmd = one_shot_command(base.path(), work.path(), None).expect("prepare + build");

        let owned = base.path().join("gemini-home");
        let home = cmd
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new("GEMINI_CLI_HOME"))
            .and_then(|(_, v)| v)
            .expect("GEMINI_CLI_HOME must be set on the one-shot too");
        assert_eq!(
            Path::new(home),
            owned,
            "D4: the owned root, not the operator's"
        );

        let args = argv(&cmd);
        let i = args
            .iter()
            .position(|a| a == "--policy")
            .unwrap_or_else(|| panic!("the one-shot must carry a policy document: {args:?}"));
        let policy = owned.join(".gemini").join("ralphy-policy.toml");
        assert_eq!(Path::new(&args[i + 1]), policy, "argv: {args:?}");
        let body = fs::read_to_string(&policy).expect("the policy document must be written");
        assert!(body.contains(r#"toolName = "invoke_agent""#), "{body}");
        assert!(body.contains(r#"decision = "deny""#), "{body}");

        let j = args
            .iter()
            .position(|a| a == "--approval-mode")
            .unwrap_or_else(|| panic!("autonomy must be requested: {args:?}"));
        assert_eq!(args[j + 1], "yolo", "argv: {args:?}");

        // AC5: the turn lands in the SAME session store a queue run writes, under
        // the same owned root, so `ralphy usage` reads one store and not two.
        let k = args
            .iter()
            .position(|a| a == "--session-id")
            .unwrap_or_else(|| panic!("the one-shot must be an addressable session: {args:?}"));
        let id = &args[k + 1];
        assert_eq!(id.len(), 36, "a v4 uuid: {id}");
        assert_eq!(id.matches('-').count(), 4, "a v4 uuid: {id}");
    }

    /// AC4's ordering, with the discriminating control: a managed host prints the
    /// tool-server notice in EVERY log, so a ladder that let any revocation
    /// pre-empt the limit would permanently misroute quota exhaustion — and a hard
    /// stop must still outrank the limit, because it will not heal on a retry.
    #[test]
    fn the_one_shot_ladder_orders_revocation_before_limit() {
        const BOTH: &str = "Gemini CLI is not running in a trusted directory.\n\
                            Error: quota exceeded for this project\n";
        let msg = one_shot_stop(BOTH, Some(55), false).expect("a stop is reported");
        assert!(
            msg.contains("refused the workspace as untrusted"),
            "the hard stop must win: {msg}"
        );
        assert!(
            !msg.contains("provider limit"),
            "the limit note must not be what is reported: {msg}"
        );

        // The limit alone still routes as the limit.
        let limit = one_shot_stop("Error: quota exceeded\n", Some(1), false).expect("a stop");
        assert!(limit.contains("provider limit"), "{limit}");

        // The exit taxonomy alone, verbatim — the reason this path cannot go
        // through a runner that discards the child's status.
        assert_eq!(
            one_shot_stop("", Some(54), false).as_deref(),
            outcome::classify_exit(Some(54)).actionable_stop(),
            "exit 54 must carry its own diagnosis"
        );
        assert!(one_shot_stop("", Some(54), false)
            .unwrap()
            .contains("exit 54"));

        // A clean exit with nothing to report is not an error.
        assert_eq!(one_shot_stop("", Some(0), false), None);
        // …and a wall timeout with no other signal still is.
        assert!(one_shot_stop("", None, true)
            .expect("a timeout is a stop")
            .contains("wall timeout"));
    }
}
