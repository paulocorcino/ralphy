//! The Codex CLI adapter: drives `codex exec` behind the core [`Agent`] contract.
//! Everything Codex-specific — the binary, the model and reasoning-effort flags,
//! the headless invocation, and the signal→[`Outcome`] mapping — is confined here.
//! See docs/adr/0004.
//!
//! Unlike the Claude adapter (a live PTY session with a Stop-hook flag file),
//! Codex needs no interactive session: `plan` and `execute` both run headless
//! `codex exec` with the prompt piped on stdin, and completion is detected from
//! Codex-native signals — the `RALPHY_DONE_EXIT`/`RALPHY_BLOCKED_EXIT` sentinels
//! in the `-o` final-message file, the process exit code, and a HEAD-diff commit
//! check — mapped onto the same core [`Outcome`].

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ralphy_adapter_support::{list_session_files, run_json_session, JsonSession, PROMPT_EXECUTE};
use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, git, plan, Agent,
    DiagnosisReport, DraftRequest, Execution, Issue, IssuesDraft, Plan, PlanLimit, TriageDraft,
    TriageRequest, Workspace,
};
use tracing::info;

mod auth;
mod command;
mod outcome;
mod skills;
mod usage;
use auth::{is_codex_auth_error, is_codex_limit_text, parse_codex_reset_hint, CODEX_AUTH_ERROR_MSG};
use command::{
    build_codex_command, build_codex_init_command, codex_config_model, resolve_init_model,
    tier_to_effort, recommended_tier, DEFAULT_CODEX_MODEL,
};
use outcome::classify_codex_outcome;
use skills::materialize_codex_skills;
use usage::{codex_sessions_dir, fold_rollout_usage};

/// The Codex planning prompt, embedded so the binary is self-contained as a global
/// tool. A variant of `prompt.plan.md` that emits a vendor-neutral
/// `low|medium|high` complexity tier (mapped to reasoning effort) instead of a
/// Claude model name. Copied to `.ralphy/plan-charter.md` for the live session
/// to read; only a one-line pointer is piped on stdin. Single source of truth
/// lives at `assets/prompts/`.
const PROMPT_PLAN_CODEX: &str = include_str!("../../../assets/prompts/prompt.plan.codex.md");

/// Drives the `codex` CLI. `model` is the operator override (else
/// [`DEFAULT_CODEX_MODEL`]); `run_dir` is where the captured logs live;
/// `max_minutes_per_issue` is the per-issue wall budget, clamped to `run_deadline`
/// when the run carries a global deadline.
pub struct CodexAgent {
    model: Option<String>,
    run_dir: PathBuf,
    max_minutes_per_issue: u64,
    run_deadline: Option<Instant>,
}

impl CodexAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            model,
            run_dir,
            max_minutes_per_issue: ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE,
            run_deadline: None,
        }
    }

    /// Set the per-issue wall-clock budget in minutes (mirrors `ClaudeAgent::with_max_minutes_per_issue`).
    pub fn with_max_minutes_per_issue(mut self, minutes: u64) -> Self {
        self.max_minutes_per_issue = minutes;
        self
    }

    /// Set the run's global wall-clock deadline (from `--deadline-hours`). Each
    /// issue's budget is then clamped to it, so an issue started just under the
    /// global limit can't overrun by a whole per-issue window (mirrors
    /// `ClaudeAgent::with_run_deadline`).
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.run_deadline = run_deadline;
        self
    }

    /// The deadline for the current issue: the per-issue budget, clamped to the
    /// run's global deadline when one is set. A budget of `0` disables the
    /// per-issue cap — the issue is then bounded only by the run deadline (or the
    /// far-future [`ralphy_core::UNBOUNDED_ISSUE_HORIZON`] when none is set).
    fn issue_deadline(&self) -> Instant {
        ralphy_adapter_support::issue_deadline(
            Instant::now(),
            self.max_minutes_per_issue,
            self.run_deadline,
            ralphy_core::UNBOUNDED_ISSUE_HORIZON,
        )
    }

    /// The single model decision point, in precedence order: the explicit
    /// `--exec-model` override, then the `model` from the user's Codex config, then
    /// [`DEFAULT_CODEX_MODEL`]. Honouring the config means a ChatGPT-auth account —
    /// which rejects `gpt-5-codex` — picks up the model it is actually entitled to
    /// with no explicit flag. Codex routes complexity by reasoning effort, not a
    /// model swap (ADR-0004 D3), so this stays a single value.
    fn resolve_model(&self) -> String {
        if let Some(m) = self.model.as_deref() {
            return m.to_string();
        }
        codex_config_model().unwrap_or_else(|| DEFAULT_CODEX_MODEL.to_string())
    }
}

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
    fs::create_dir_all(neutral_cwd).ok();
    let out_path = neutral_cwd.join("diagnosis.json");
    // A stale report from a prior run must never masquerade as this session's
    // output, so clear it before the session runs.
    let _ = fs::remove_file(&out_path);

    let model = resolve_init_model(model);
    let effort = effort.unwrap_or("medium");
    let prompt = build_diagnose_prompt(repo, &out_path);

    info!(%model, effort, "diagnosing repo with codex exec");
    let cmd = build_codex_init_command(&model, effort, neutral_cwd);
    let log_path = neutral_cwd.join("diagnose.log");
    run_json_session(
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
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    // A stale draft from a prior run must never masquerade as this session's
    // output, so clear it before the session runs.
    let _ = fs::remove_file(out_path);

    let model = resolve_init_model(model);
    let effort = effort.unwrap_or("medium");
    let prompt =
        build_init_issues_prompt(repo, req.mode, req.source_docs, req.triage_label, out_path);

    info!(%model, effort, mode = req.mode.as_str(), "drafting issues with codex exec");
    let cmd = build_codex_init_command(&model, effort, repo);
    let log_path = repo.join(".ralphy").join("init-issues.log");
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    run_json_session(
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
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    // A stale draft from a prior run must never masquerade as this session's
    // output, so clear it before the session runs.
    let _ = fs::remove_file(out_path);

    let model = resolve_init_model(model);
    let effort = effort.unwrap_or("medium");
    let prompt = build_triage_prompt(repo, req.issue_numbers, req.queue_label, out_path);

    info!(%model, effort, "triaging issues with codex exec");
    let cmd = build_codex_init_command(&model, effort, repo);
    let log_path = repo.join(".ralphy").join("triage.log");
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    run_json_session(
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

impl Agent for CodexAgent {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn plan(&self, _issue: &Issue, ws: &Workspace) -> Result<Plan> {
        fs::create_dir_all(ws.ralphy_dir()).ok();
        fs::create_dir_all(&self.run_dir).ok();
        materialize_codex_skills(ws)?;

        let plan_path = ws.plan_path();
        // Plan fresh every run; never reuse a stale artifact.
        let _ = fs::remove_file(&plan_path);

        // Full charter on disk (mirrors .ralphy/exec.md); rewritten each plan
        // call so a resumed session still finds it.
        fs::write(ws.plan_charter_path(), PROMPT_PLAN_CODEX)
            .context("writing .ralphy/plan-charter.md")?;

        let model = self.resolve_model();
        let out_path = ws.ralphy_dir().join("codex-last.txt");
        let _ = fs::remove_file(&out_path);

        // Snapshot the rollout tree around the call: a file that APPEARED is this
        // run's session, while one that merely grew is a concurrent pre-existing
        // session and is excluded (ADR-0008 D10, appeared-over-grew).
        let sessions_dir = codex_sessions_dir();
        let before = sessions_dir
            .as_deref()
            .map(|d| list_session_files(d, "jsonl", true, Some("rollout-")))
            .unwrap_or_default();

        // Planning always runs at `high` effort (ADR-0004 D3).
        let cmd = build_codex_command(&model, "high", ws.repo_root(), &out_path);
        let timeout = self
            .issue_deadline()
            .saturating_duration_since(Instant::now());
        info!(model = %model, effort = "high", "planning with codex exec");
        let (_, _, log) = self.run_codex(cmd, ralphy_adapter_support::PLAN_CHARTER, timeout)?;
        let after = sessions_dir
            .as_deref()
            .map(|d| list_session_files(d, "jsonl", true, Some("rollout-")))
            .unwrap_or_default();

        if !plan_path.exists() {
            // A usage limit during planning is not a generic failure: surface it
            // as a typed `PlanLimit` (with the parsed reset hint) so the runner
            // routes it through the same stop-and-report / auto-resume path as an
            // execute-time `Outcome::Limit`, rather than aborting the whole run.
            if let Some(reset) = ralphy_adapter_support::detect_limit(
                &log,
                is_codex_limit_text,
                parse_codex_reset_hint,
            ) {
                return Err(PlanLimit { reset }.into());
            }
            // An auth failure won't self-heal (unlike a usage limit), so stop the
            // run with an actionable message instead of a generic "no plan".
            if is_codex_auth_error(&log) {
                bail!(
                    "{CODEX_AUTH_ERROR_MSG} (see {})",
                    self.run_dir.join("codex.log").display()
                );
            }
            bail!(
                "codex produced no plan at {} (see {})",
                plan_path.display(),
                self.run_dir.join("codex.log").display()
            );
        }
        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            recommended_model: recommended_tier(&md),
            path: plan_path,
            usage: fold_rollout_usage(&before, &after, Some(model)),
        })
    }

    fn execute(&self, plan: &Plan, ws: &Workspace) -> Result<Execution> {
        fs::create_dir_all(&self.run_dir).ok();
        fs::create_dir_all(ws.ralphy_dir()).ok();
        materialize_codex_skills(ws)?;

        let model = self.resolve_model();
        // Execution takes the plan's neutral complexity tier as reasoning effort.
        let effort = tier_to_effort(plan.recommended_model.as_deref());
        let out_path = ws.ralphy_dir().join("codex-last.txt");
        let _ = fs::remove_file(&out_path);

        // HEAD before/after bounds the work this call committed (progress guard).
        let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        // Snapshot the rollout tree around the call for appeared-over-grew token
        // capture (ADR-0008 D10), the same rule the Claude adapter uses.
        let sessions_dir = codex_sessions_dir();
        let before = sessions_dir
            .as_deref()
            .map(|d| list_session_files(d, "jsonl", true, Some("rollout-")))
            .unwrap_or_default();
        let timeout = self
            .issue_deadline()
            .saturating_duration_since(Instant::now());
        let cmd = build_codex_command(&model, effort, ws.repo_root(), &out_path);
        info!(model = %model, effort, "executing with codex exec");
        let (exited_cleanly, timed_out, log) = self.run_codex(cmd, PROMPT_EXECUTE, timeout)?;
        let after = sessions_dir
            .as_deref()
            .map(|d| list_session_files(d, "jsonl", true, Some("rollout-")))
            .unwrap_or_default();

        // A signed-out account never makes progress: stop the run with an
        // actionable message rather than letting it fall through to `Stuck`.
        if is_codex_auth_error(&log) {
            bail!(
                "{CODEX_AUTH_ERROR_MSG} (see {})",
                self.run_dir.join("codex.log").display()
            );
        }

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let out = fs::read_to_string(&out_path).unwrap_or_default();

        let outcome = classify_codex_outcome(exited_cleanly, timed_out, committed, &out, &log);
        info!(
            ?outcome,
            exited_cleanly, timed_out, committed, "codex execution ended"
        );
        Ok(Execution {
            outcome,
            usage: fold_rollout_usage(&before, &after, Some(model)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── with_max_minutes_per_issue ──────────────────────────────────────────

    #[test]
    fn codex_honours_max_minutes_per_issue() {
        assert_eq!(
            CodexAgent::new(None, PathBuf::from("/run")).max_minutes_per_issue,
            ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE
        );
        let a = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(120);
        assert_eq!(a.max_minutes_per_issue, 120);
        let short = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1);
        let long = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(long.issue_deadline() > short.issue_deadline());
        let rd = Instant::now() + Duration::from_secs(1);
        let clamped = CodexAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(1000)
            .with_run_deadline(Some(rd));
        assert!(clamped.issue_deadline() <= rd);
    }

    #[test]
    fn codex_zero_minutes_disables_the_per_issue_cap() {
        // `0` → no per-issue cap: the deadline sits at the far-future horizon,
        // well past any finite budget.
        let uncapped = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(0);
        let capped = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(uncapped.issue_deadline() > capped.issue_deadline());

        // …but an uncapped issue is still bounded by the run deadline when set.
        let rd = Instant::now() + Duration::from_secs(1);
        let bounded = CodexAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(0)
            .with_run_deadline(Some(rd));
        assert!(bounded.issue_deadline() <= rd);
    }

    // ── resolve_model ───────────────────────────────────────────────────────

    #[test]
    fn resolve_model_override_wins() {
        // The explicit --exec-model override wins over config and default, with no
        // dependence on the machine's Codex config.
        let overridden = CodexAgent::new(Some("gpt-5".into()), PathBuf::from("/run"));
        assert_eq!(overridden.resolve_model(), "gpt-5");
    }

    // ── trait binding (compile-level) ───────────────────────────────────────

    #[test]
    fn codex_agent_is_a_dyn_agent() {
        // Proves `CodexAgent: Agent` and that it can be handed to the core as a
        // `&dyn Agent` (the core never learns the vendor).
        let agent = CodexAgent::new(None, PathBuf::from("/run"));
        let _as_dyn: &dyn Agent = &agent;
    }

    // ── PROMPT_PLAN_CODEX reviewer step ────────────────────────────────────

    #[test]
    fn plan_charter_file_carries_full_prompt() {
        // The full charter lands on disk (mirrors exec.md) and per-issue stdin
        // stays a one-line pointer — pins the byte reduction issue #80 delivers.
        let base =
            std::env::temp_dir().join(format!("ralphy-codex-charter-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);
        fs::create_dir_all(ws.ralphy_dir()).unwrap();

        fs::write(ws.plan_charter_path(), PROMPT_PLAN_CODEX).unwrap();
        assert_eq!(
            fs::read_to_string(ws.plan_charter_path()).unwrap(),
            PROMPT_PLAN_CODEX
        );
        assert!(ralphy_adapter_support::PLAN_CHARTER.len() * 50 < PROMPT_PLAN_CODEX.len());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn prompt_plan_codex_contains_reviewer_step() {
        assert!(
            PROMPT_PLAN_CODEX.contains("reviewer"),
            "planning prompt must reference the reviewer skill"
        );
        let lower = PROMPT_PLAN_CODEX.to_lowercase();
        assert!(
            lower.contains("only") && lower.contains("commits you made"),
            "reviewer step must scope to this issue's own commits"
        );
        assert!(
            !PROMPT_PLAN_CODEX.contains("independent subagent"),
            "must not use Claude 'independent subagent' phrasing"
        );
    }
}
