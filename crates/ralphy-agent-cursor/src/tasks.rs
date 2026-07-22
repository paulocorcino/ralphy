//! One-shot headless `cursor-agent` sessions for the `init`/`triage` flows
//! (ADR-0012 stages 2 & 8, ADR-0017, ADR-0042) — repo diagnosis, backlog → issues
//! drafting, agent-triage drafting, and knowledge consolidation. None of these
//! publish to GitHub; the cli applies the drafted artifact after the operator
//! confirms.
//!
//! D6 is not a run-loop guarantee, it is an ADAPTER one: a one-shot walks the same
//! repository a run does, so every verb here calls [`one_shot_preflight`] as its
//! FIRST statement — before any prompt is built, any file is created, any child is
//! spawned. The same call seeds D17's scratch `CURSOR_CONFIG_DIR`, so a one-shot
//! never rewrites the operator's own `cli-config.json` either.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::info;

use ralphy_adapter_support::{run_init_session, run_text_session, JsonSession, TextSession};
use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, DiagnosisReport,
    DraftRequest, IssuesDraft, Settings, TriageDraft, TriageRequest, Usage, Workspace,
    PROMPT_CONSOLIDATE,
};

use crate::usage::parse_cursor_usage;

use crate::auth::{is_cursor_auth_error, CURSOR_AUTH_ERROR_MSG};
use crate::command::{build_cursor_init_command, operator_config_dir, seed_cursor_config_dir};
use crate::guards::indexing_gate;
use crate::settings::CursorSettings;

/// D6's opt-in, read from the TARGET repo's `.ralphy/settings.json`.
///
/// The one-shot signatures are shared across every vendor, so the flag cannot ride
/// a parameter — it is read here instead. Fail-closed at both hops: an unreadable
/// or malformed settings file yields `false`, i.e. the refusal, never the upload.
fn allow_indexing(repo: &Path) -> bool {
    Settings::load(&Workspace::new(repo))
        .unwrap_or_default()
        .agent_settings::<CursorSettings>(CursorSettings::SECTION)
        .unwrap_or_default()
        .allow_codebase_indexing_i_understand_the_risk
}

/// The gate + isolation pair every one-shot pays before it does anything else.
///
/// `work_dir` is the CHILD's cwd, which is what the indexing service walks — for
/// `diagnose_repo` that is the neutral directory outside the repo, not the repo
/// being diagnosed. `repo` is where the opt-in is persisted. `config_dir` is this
/// verb's scratch `CURSOR_CONFIG_DIR`, seeded only once the gate has passed: a
/// refused one-shot leaves nothing behind.
fn one_shot_preflight(work_dir: &Path, repo: &Path, config_dir: &Path) -> Result<()> {
    indexing_gate(work_dir, allow_indexing(repo))?;
    seed_cursor_config_dir(operator_config_dir().as_deref(), config_dir)
}

/// The scratch config dir's name, mirroring `CursorAgent::config_dir` (D17). It
/// sits beside each verb's own log, so it is inspectable and scoped to that verb.
const CONFIG_DIR_NAME: &str = "cursor-config";

/// The spawn-failure sentence every verb shares: this vendor is on `PATH` under
/// neither of its two names (D14), so "is it installed?" is the honest question.
const SPAWN_ERR: &str = "failed to spawn the `cursor-agent` CLI (is it installed?)";

/// Run a one-shot headless `cursor-agent` repo-diagnosis session (ADR-0012 stage 2)
/// from `neutral_cwd` — a directory OUTSIDE the target repo. The target `repo` is
/// passed as data in the prompt; the session writes its JSON report to
/// `<neutral_cwd>/diagnosis.json`, which this function reads, validates against
/// [`DiagnosisReport`], and returns.
///
/// The gate runs on `neutral_cwd`, not `repo`: nothing under `repo` is uploaded by
/// a child that never enters it, and gating on the target instead would refuse a
/// verb whose whole point is reading a repository it does not open. `effort` is
/// unused — Cursor has no reasoning-effort axis (ADR-0042 D5).
pub fn diagnose_repo(
    repo: &Path,
    neutral_cwd: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<DiagnosisReport> {
    let _ = effort;
    let config_dir = neutral_cwd.join(CONFIG_DIR_NAME);
    one_shot_preflight(neutral_cwd, repo, &config_dir)?;

    let out_path = neutral_cwd.join("diagnosis.json");
    let prompt = build_diagnose_prompt(repo, &out_path);
    let log_path = neutral_cwd.join("diagnose.log");

    info!(?model, "diagnosing repo with cursor");
    let cmd = build_cursor_init_command(model, neutral_cwd, &config_dir);
    run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path: &out_path,
            spawn_err: SPAWN_ERR,
            auth_msg: CURSOR_AUTH_ERROR_MSG,
            timeout_msg: "diagnosis session hit the wall timeout",
            missing_msg: "diagnosis session left no report",
        },
        is_cursor_auth_error,
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

/// Run a one-shot headless `cursor-agent` backlog/milestone → issues session
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
    let config_dir = repo.join(".ralphy").join(CONFIG_DIR_NAME);
    one_shot_preflight(repo, repo, &config_dir)?;

    let prompt =
        build_init_issues_prompt(repo, req.mode, req.source_docs, req.triage_label, out_path);
    let log_path = repo.join(".ralphy").join("init-issues.log");

    info!(
        ?model,
        mode = req.mode.as_str(),
        "drafting issues with cursor"
    );
    let cmd = build_cursor_init_command(model, repo, &config_dir);
    run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: SPAWN_ERR,
            auth_msg: CURSOR_AUTH_ERROR_MSG,
            timeout_msg: "backlog → issues session hit the wall timeout",
            missing_msg: "issues session left no draft",
        },
        is_cursor_auth_error,
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

/// Run a one-shot headless `cursor-agent` agent-triage session (ADR-0017). Mirrors
/// [`draft_issues`] but drives the triage charter over each `triage-agent` issue's
/// body + full comment thread, writing a [`TriageDraft`] JSON to `out_path` for the
/// cli to apply after the operator confirms. Never publishes to GitHub.
/// `req.image_paths` is unused: no attachment channel exists anywhere in this
/// vendor's headless surface (`ACCEPTS_IMAGES = false`, ADR-0042 D15).
pub fn triage_issues(
    repo: &Path,
    out_path: &Path,
    req: &TriageRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<TriageDraft> {
    let _ = effort;
    let config_dir = repo.join(".ralphy").join(CONFIG_DIR_NAME);
    one_shot_preflight(repo, repo, &config_dir)?;

    let prompt = format!(
        "{}{}",
        build_triage_prompt(repo, req.issue_numbers, req.queue_label, out_path),
        req.attachments_manifest
    );
    let log_path = repo.join(".ralphy").join("triage.log");

    info!(?model, "triaging issues with cursor");
    let cmd = build_cursor_init_command(model, repo, &config_dir);
    run_init_session(
        JsonSession {
            cmd,
            prompt: &prompt,
            timeout,
            log_path: &log_path,
            out_path,
            spawn_err: SPAWN_ERR,
            auth_msg: CURSOR_AUTH_ERROR_MSG,
            timeout_msg: "triage session hit the wall timeout",
            missing_msg: "triage session left no draft",
        },
        is_cursor_auth_error,
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

/// Run a one-shot headless `cursor-agent` knowledge-consolidation session in `ws`'s
/// repo cwd: pipe the shared consolidation charter on stdin and wait up to
/// `timeout`. The session's only deliverable is the rewritten `KNOWLEDGE.md`, which
/// the caller verifies; the consumed notes are archived by the caller, not here.
///
/// Returns the invocation's [`Usage`] parsed from the same `stream-json` `result`
/// record the plan/execute path reads (ADR-0042 D11): the one-shot builder carries
/// `--output-format stream-json` exactly like the run builder, so the consolidation
/// pass is a real, countable vendor call the caller folds into the run total and the
/// ledger (issue #269) rather than dropping.
pub fn consolidate_knowledge(
    ws: &Workspace,
    run_dir: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<Usage> {
    let _ = effort;
    let config_dir = run_dir.join(CONFIG_DIR_NAME);
    one_shot_preflight(ws.repo_root(), ws.repo_root(), &config_dir)?;

    std::fs::create_dir_all(run_dir).ok();
    let log_path = run_dir.join("consolidate.log");

    info!(?model, "consolidating knowledge with cursor");
    let cmd = build_cursor_init_command(model, ws.repo_root(), &config_dir);
    let log = run_text_session(
        TextSession {
            cmd,
            prompt: PROMPT_CONSOLIDATE,
            timeout,
            log_path: &log_path,
            spawn_err: SPAWN_ERR,
            auth_msg: CURSOR_AUTH_ERROR_MSG,
            timeout_msg: "consolidation session hit the wall timeout",
        },
        is_cursor_auth_error,
    )?;
    Ok(parse_cursor_usage(&log, model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A temp directory that LOOKS like a git repository to the gate's walk.
    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        fs::create_dir(d.path().join(".git")).expect("mkdir .git");
        d
    }

    /// D6 explicitly allows this, and it is the case an over-eager implementation
    /// breaks: `draft_issues` / `consolidate_knowledge` run where there is no
    /// repository at all, and must reach the seed rather than be refused.
    #[test]
    fn a_one_shot_outside_any_repository_passes_the_preflight() {
        let d = tempfile::tempdir().unwrap();
        let config_dir = d.path().join("cursor-config");
        one_shot_preflight(d.path(), d.path(), &config_dir)
            .expect("no repository, nothing to gate");
        assert!(
            config_dir.is_dir(),
            "the preflight must continue to D17's seed once the gate passes"
        );
    }

    /// The refusal happens BEFORE the seed: a gated one-shot leaves no scratch dir.
    #[test]
    fn the_preflight_refuses_an_unprotected_repository() {
        let d = repo();
        let config_dir = d.path().join("cursor-config");
        let err = one_shot_preflight(d.path(), d.path(), &config_dir)
            .expect_err("an un-opted-out repository must refuse the one-shot");
        let msg = err.to_string();
        assert!(msg.contains(".cursorindexingignore"), "{msg}");
        assert!(
            msg.contains("cursor.allow_codebase_indexing_i_understand_the_risk"),
            "{msg}"
        );
        assert!(
            !config_dir.exists(),
            "a refused one-shot must not seed a config dir"
        );
    }

    /// The persisted opt-in is read from the TARGET repo, not from a parameter —
    /// the one-shot signatures are shared across every vendor.
    #[test]
    fn the_persisted_opt_in_reaches_the_one_shots() {
        let d = repo();
        fs::create_dir_all(d.path().join(".ralphy")).unwrap();
        fs::write(
            d.path().join(".ralphy").join("settings.json"),
            r#"{"cursor":{"allow_codebase_indexing_i_understand_the_risk":true}}"#,
        )
        .unwrap();
        assert!(allow_indexing(d.path()), "the persisted key must be read");
        let config_dir = d.path().join("cursor-config");
        one_shot_preflight(d.path(), d.path(), &config_dir)
            .expect("the operator's explicit opt-in must reach the capability");
        assert!(config_dir.is_dir());
    }

    /// Fail-closed at BOTH hops: a settings file that does not parse must land on
    /// the refusal, never on the upload. The operator's mistake costs them a
    /// misleading message, not their repository's contents.
    #[test]
    fn a_malformed_settings_file_fails_closed() {
        let d = repo();
        fs::create_dir_all(d.path().join(".ralphy")).unwrap();
        fs::write(d.path().join(".ralphy").join("settings.json"), "{ not json").unwrap();
        assert!(
            !allow_indexing(d.path()),
            "an unparseable settings file must not grant the opt-in"
        );

        // The other hop: valid JSON whose `cursor` section has the wrong shape.
        fs::write(
            d.path().join(".ralphy").join("settings.json"),
            r#"{"cursor":{"allow_codebase_indexing_i_understand_the_risk":"yes"}}"#,
        )
        .unwrap();
        assert!(
            !allow_indexing(d.path()),
            "a malformed cursor section must not grant the opt-in"
        );
    }

    /// D6's rule is stated over the CHILD's working directory. `diagnose_repo`'s
    /// child runs in `neutral_cwd`, so that is what the gate must be given —
    /// passing `repo` instead would refuse a verb that uploads nothing. Source pin:
    /// no test here spawns a real child, so nothing else would catch the swap.
    #[test]
    fn diagnose_gates_on_the_child_cwd_not_the_target_repo() {
        let src = include_str!("tasks.rs");
        assert!(
            src.contains(concat!("one_shot_", "preflight(neutral_cwd,")),
            "diagnose_repo must gate on the child's cwd, not the target repo"
        );
    }

    /// The behavioural fan-out: all four verbs refuse an unprotected repository
    /// before they build a prompt, create an artifact, or spawn a child.
    #[test]
    fn each_one_shot_refuses_an_unprotected_repository() {
        let d = repo();
        let repo_path = d.path();
        let out = repo_path.join("out.json");
        let short = Duration::from_secs(1);

        let mut errs: Vec<String> = Vec::new();
        errs.push(
            draft_issues(
                repo_path,
                &out,
                &DraftRequest {
                    mode: ralphy_core::IssuesMode::LooseBacklog,
                    source_docs: &[],
                    triage_label: "x",
                },
                None,
                None,
                short,
            )
            .expect_err("draft_issues must refuse")
            .to_string(),
        );
        errs.push(
            triage_issues(
                repo_path,
                &out,
                &TriageRequest {
                    issue_numbers: &[1],
                    queue_label: "AFK",
                    attachments_manifest: "",
                    image_paths: &[],
                },
                None,
                None,
                short,
            )
            .expect_err("triage_issues must refuse")
            .to_string(),
        );
        // The neutral cwd is INSIDE the repo here on purpose: that is the shape the
        // gate must catch, and `diagnose_repo` gates on it rather than on `repo`.
        let nested = repo_path.join("neutral");
        fs::create_dir_all(&nested).unwrap();
        errs.push(
            diagnose_repo(repo_path, &nested, None, None, short)
                .expect_err("diagnose_repo must refuse")
                .to_string(),
        );
        let run_dir = repo_path.join("run");
        errs.push(
            consolidate_knowledge(&Workspace::new(repo_path), &run_dir, None, None, short)
                .expect_err("consolidate_knowledge must refuse")
                .to_string(),
        );

        for msg in &errs {
            assert!(msg.contains(".cursorindexingignore"), "{msg}");
        }
        // `out_path` alone proves nothing — only the CHILD ever writes it, so it is
        // absent whether or not the gate fired. `<repo>/.ralphy` does: the shared
        // harness `create_dir_all`s each verb's log parent on its way to the spawn,
        // so the directory's absence pins that the refusal preceded the harness.
        assert!(!out.exists(), "no artifact may be created before the gate");
        assert!(
            !repo_path.join(".ralphy").exists(),
            "the refusal must precede the session harness, which creates the log dir"
        );
        for dir in [
            repo_path.join(".ralphy").join("cursor-config"),
            nested.join("cursor-config"),
            run_dir.join("cursor-config"),
        ] {
            assert!(
                !dir.exists(),
                "a refused one-shot must seed nothing: {}",
                dir.display()
            );
        }
    }
}
