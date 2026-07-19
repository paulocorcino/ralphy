//! The OpenCode CLI adapter: drives `opencode run` behind the core [`Agent`]
//! contract. Everything OpenCode-specific — the binary, the model/variant flags,
//! the headless invocation, the line-delimited-JSON event stream, and the
//! signal→[`Outcome`] mapping — is confined here. See docs/adr/0005.
//!
//! Like the Codex adapter (and unlike Claude's live PTY session), OpenCode needs
//! no interactive session: `plan` and `execute` both run headless `opencode run`
//! with the prompt piped on stdin, and completion is detected from the
//! `RALPHY_DONE_EXIT`/`RALPHY_BLOCKED_EXIT` sentinels parsed out of the JSON
//! `text` parts, a JSON `error` event, the process exit code, and a HEAD-diff
//! commit check — mapped onto the same core [`Outcome`].
//!
//! Skills materialization (ADR-0005 D7) is implemented here: before every `plan`
//! and `execute` call the embedded skills tree is extracted to `<repo>/.ralphy/skills`
//! and the absolute path is injected as `OPENCODE_CONFIG_CONTENT` so OpenCode's
//! `skills.paths` config key points at it. Auth-error (D6) is implemented:
//! `is_opencode_auth_error` detects `ProviderAuthError` in the combined log and an
//! actionable bail fires in both `plan` and `execute` before any generic
//! classification. Usage-limit (D9) is implemented: `parse_opencode_limit` scans
//! the JSON stream for a 429/`APIError` or documented rate-limit string (and
//! `parse_opencode_log_limit` the logfmt server log for quota blocks that never
//! reach the JSON stream), and `classify_opencode_outcome` upgrades
//! `Timeout`/`Stuck` to `Outcome::Limit` when one is seen. Any reset hint is then
//! **discarded** (`unschedulable_opencode_limit`): OpenCode fronts many providers
//! whose reset strings are unreliable (FinCal #73), so the runner never schedules
//! on one and instead retries on the ~30-min synthetic cadence (ADR-0030 D1).
//! Auto-resume is the default for all agents; `--stop-on-limit` is the opt-out
//! (ADR-0030 D3) — it is no longer forced for OpenCode.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use ralphy_adapter_support::{run_exec_session, run_plan_session, ExecCfg, IssueBudget, PlanCfg};
use ralphy_core::{git, plan, Agent, Execution, Issue, Plan, Workspace};
use tracing::info;

mod command;
mod events;
mod outcome;
mod skills;
mod tasks;
mod usage;

/// OpenCode's headless `run` has no image-input channel, so images are never
/// fetched for it — the manifest shows `not fetched (opencode has no image
/// input)` (ADR-0025 §4). A future vision OpenCode flips this in its own crate.
pub const ACCEPTS_IMAGES: bool = false;

use command::build_opencode_command;
use events::{
    is_opencode_auth_error, parse_opencode_events, parse_opencode_limit, parse_opencode_log_limit,
    usage_limit_regex, OPENCODE_AUTH_ERROR_MSG,
};
use outcome::{classify_opencode_outcome, unschedulable_opencode_limit};
use skills::{materialize_opencode_skills, opencode_skills_config};
pub use tasks::{consolidate_knowledge, diagnose_repo, draft_issues, list_models, triage_issues};
use usage::{opencode_usage, resolved_model_label, session_id_from_stream};

/// The OpenCode planning prompt, embedded so the binary is self-contained as a
/// global tool. A variant of `prompt.plan.md` with the `## Execution model` tier
/// line removed (OpenCode drops complexity routing, ADR-0005 D3/D8a) and the
/// reviewer step committed to the **inline `reviewer` skill** — auto-discovered
/// via `skills.paths`, **not** a subagent. Headless custom-subagent dispatch is
/// blocked upstream (`opencode#29616`/`#20059`: Task tool `subagent_type` is
/// hardcoded to `explore`/`general`), so the inline skill is the only working
/// headless mechanism (ADR-0005 D8). Copied to `.ralphy/plan-charter.md` for
/// the live session to read; only a one-line pointer is piped on stdin. Single
/// source of truth lives at `assets/prompts/`.
const PROMPT_PLAN_OPENCODE: &str = include_str!("../../../assets/prompts/prompt.plan.opencode.md");

/// OpenCode-specific settings persisted under the [`OpenCodeSettings::SECTION`]
/// section of `.ralphy/settings.json` (ADR-0010). The core stores the section as
/// opaque JSON; this adapter owns the schema (ADR-0002 amendment, #79).
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OpenCodeSettings {
    /// The model id to pass as `-m <id>` when no `--exec-model` flag is given.
    /// `None` / empty → OpenCode resolves the model itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl OpenCodeSettings {
    /// The settings-file section this struct lives under.
    pub const SECTION: &'static str = "opencode";
}

/// Drives the `opencode` CLI. `model` is the operator override (omitted entirely
/// when `None`, deferring to OpenCode's own resolution, ADR-0005 D4); `variant`
/// is the operator's optional effort knob, passed through only when set (D3);
/// `run_dir` is where the captured logs live; `max_minutes_per_issue` is the
/// per-issue wall budget, clamped to `run_deadline` when the run carries a global
/// deadline.
pub struct OpenCodeAgent {
    model: Option<String>,
    variant: Option<String>,
    run_dir: PathBuf,
    budget: IssueBudget,
}

impl OpenCodeAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            model,
            variant: None,
            run_dir,
            budget: IssueBudget::new(ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE),
        }
    }

    /// Set the operator's optional `--variant` knob (ADR-0005 D3). Passed through
    /// to OpenCode only when present; omitted otherwise so the adapter never
    /// sends a value the provider rejects.
    pub fn with_variant(mut self, variant: Option<String>) -> Self {
        self.variant = variant;
        self
    }

    /// Set the per-issue wall-clock budget in minutes (mirrors `ClaudeAgent::with_max_minutes_per_issue`).
    pub fn with_max_minutes_per_issue(mut self, minutes: u64) -> Self {
        self.budget = self.budget.with_max_minutes_per_issue(minutes);
        self
    }

    /// Set the idle watchdog window in minutes: reap the child after that long
    /// with no output at all. `0` disables it. Unlike the per-issue cap, this
    /// keys on progress rather than elapsed time (docs/adr/0038).
    pub fn with_idle_minutes(mut self, minutes: u64) -> Self {
        self.budget = self.budget.with_idle_minutes(minutes);
        self
    }

    /// Set the run's global wall-clock deadline (from `--deadline-hours`). Each
    /// issue's budget is then clamped to it, so an issue started just under the
    /// global limit can't overrun by a whole per-issue window (mirrors
    /// `CodexAgent::with_run_deadline`).
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.budget = self.budget.with_run_deadline(run_deadline);
        self
    }

    /// The deadline for the current issue: the per-issue budget, clamped to the
    /// run's global deadline when one is set. A budget of `0` disables the
    /// per-issue cap — the issue is then bounded only by the run deadline (or the
    /// far-future [`ralphy_core::UNBOUNDED_ISSUE_HORIZON`] when none is set).
    /// The plan/execute paths read the budget directly (`self.budget.timeout`);
    /// this stays as the deadline oracle the budget tests assert against.
    #[cfg(test)]
    fn issue_deadline(&self) -> Instant {
        self.budget.deadline(ralphy_core::UNBOUNDED_ISSUE_HORIZON)
    }
}

impl Agent for OpenCodeAgent {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        let plan_path = ws.plan_path();
        let log_path = self.run_dir.join("opencode.log");

        let run = || {
            let skills_dir = materialize_opencode_skills(ws)?;
            let skills_config = opencode_skills_config(&skills_dir);
            let cmd = build_opencode_command(
                self.model.as_deref(),
                self.variant.as_deref(),
                ws.repo_root(),
                &skills_config,
            );
            info!(model = ?self.model, variant = ?self.variant, "planning with opencode run");
            // Clock the budget at the spawn, not method entry, so the run_deadline
            // clamp isn't eroded by the preceding dir/skills setup.
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            // No early-kill on plan: plan surfaces no usage limit today (the detector
            // below is `|_log| None`), so there is nothing for a kill to hand off to.
            // The call still streams the log live, like execute.
            let r =
                self.run_opencode(cmd, ralphy_adapter_support::PLAN_CHARTER, timeout, |_| {
                    false
                })?;
            let stdout = r.stdout.clone();
            Ok((r, stdout))
        };

        let ralphy_dir = ws.ralphy_dir();
        let charter_path = ws.plan_charter_path();
        let session = run_plan_session(
            PlanCfg {
                issue_number: issue.number,
                ralphy_dir: &ralphy_dir,
                run_dir: &self.run_dir,
                plan_path: &plan_path,
                plan_charter_path: &charter_path,
                charter_body: PROMPT_PLAN_OPENCODE,
                log_path: &log_path,
                auth_msg: OPENCODE_AUTH_ERROR_MSG,
                no_plan_msg: "opencode produced no plan",
            },
            run,
            is_opencode_auth_error,
            // No plan-time usage limit is surfaced for OpenCode today (the current
            // ladder is auth-then-generic); the limit path lives in execute (D9).
            |_log| None,
        )?;

        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        // None = resumed (finalized plan kept, no vendor run): no stdout to parse,
        // so report zero planning tokens.
        let (usage, session_id) = match session {
            Some((_, stdout_text)) => (
                opencode_usage(&stdout_text),
                session_id_from_stream(&stdout_text),
            ),
            None => (ralphy_core::Usage::default(), None),
        };
        info!(
            model = resolved_model_label(&usage),
            "opencode plan resolved model"
        );
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            // OpenCode drops complexity routing (ADR-0005 D3), so there is no tier.
            recommended_model: None,
            path: plan_path,
            usage,
            session_id,
        })
    }

    fn execute(&self, _plan: &Plan, ws: &Workspace) -> Result<Execution> {
        let log_path = self.run_dir.join("opencode.log");
        // HEAD before/after bounds the work this call committed (progress guard).
        let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();

        let run = || {
            let skills_dir = materialize_opencode_skills(ws)?;
            let skills_config = opencode_skills_config(&skills_dir);
            let cmd = build_opencode_command(
                self.model.as_deref(),
                self.variant.as_deref(),
                ws.repo_root(),
                &skills_config,
            );
            info!(model = ?self.model, variant = ?self.variant, "executing with opencode run");
            // Clock the budget at the spawn, not method entry, so the run_deadline
            // clamp isn't eroded by the preceding dir/skills setup.
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            // Early-kill: a provider quota block only ever prints to `--print-logs`
            // stderr (never the JSON stream), then the child idles in silent backoff
            // until the wall timeout (FinCal #71/#73, glm-5.2). Match the same
            // usage-limit shape the post-run `parse_opencode_log_limit` keys on, so an
            // early-killed run classifies as `Limit` identically — only ~sub-second
            // instead of burning the whole per-issue budget.
            let limit_re = usage_limit_regex();
            let r = self.run_opencode(
                cmd,
                ralphy_adapter_support::PROMPT_EXECUTE,
                timeout,
                move |line| limit_re.is_match(line),
            )?;
            let stdout = r.stdout.clone();
            Ok((r, stdout))
        };

        let ralphy_dir = ws.ralphy_dir();
        let (r, stdout_text) = run_exec_session(
            ExecCfg {
                ralphy_dir: &ralphy_dir,
                run_dir: &self.run_dir,
                log_path: &log_path,
                auth_msg: OPENCODE_AUTH_ERROR_MSG,
            },
            run,
            is_opencode_auth_error,
        )?;

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let (text, saw_error) = parse_opencode_events(&stdout_text);
        // Prefer the JSON-event limit (structured, carries reset hints); fall back to
        // the logfmt scan over the combined stdout+stderr log for providers whose
        // quota block only prints to `--print-logs` stderr and never reaches the JSON
        // stream (Z.ai `zai-coding-plan`/GLM, kimi — D9, FinCal #71).
        let detected =
            parse_opencode_limit(&stdout_text).or_else(|| parse_opencode_log_limit(&r.log));
        // OpenCode fronts N providers whose reset strings are unreliable: a "5 hour"
        // limit reporting a reset ~12h out (FinCal #73, glm-5.2) is internally
        // impossible, yet parses cleanly and would park the run on a bogus instant.
        // Never schedule on an OpenCode reset — keep detecting the limit but discard
        // the hint so the runner falls to the ~30-min synthetic cadence (ADR-0030 D1)
        // instead of honouring it. Trustworthiness is a per-vendor extraction decision
        // (the `classify` seam, ADR-0023); Claude/Codex are stable and keep their hints.
        if let Some(Some(reset)) = &detected {
            info!(
                %reset,
                "opencode reported a usage-limit reset — discarding (OpenCode resets are unschedulable by policy)"
            );
        }
        let limit = unschedulable_opencode_limit(detected);

        let outcome = classify_opencode_outcome(
            r.exited_cleanly,
            r.timed_out,
            committed,
            &text,
            saw_error,
            limit,
        );
        let usage = opencode_usage(&stdout_text);
        info!(
            ?outcome,
            model = resolved_model_label(&usage),
            exited_cleanly = r.exited_cleanly,
            timed_out = r.timed_out,
            committed,
            saw_error,
            "opencode execution ended"
        );
        Ok(Execution {
            outcome,
            usage,
            session_id: session_id_from_stream(&stdout_text),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    // ── with_max_minutes_per_issue ──────────────────────────────────────────

    #[test]
    fn opencode_honours_max_minutes_per_issue() {
        assert_eq!(
            OpenCodeAgent::new(None, PathBuf::from("/run"))
                .budget
                .max_minutes_per_issue,
            ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE
        );
        let a = OpenCodeAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(120);
        assert_eq!(a.budget.max_minutes_per_issue, 120);
        let short = OpenCodeAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1);
        let long = OpenCodeAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(long.issue_deadline() > short.issue_deadline());
        let rd = Instant::now() + Duration::from_secs(1);
        let clamped = OpenCodeAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(1000)
            .with_run_deadline(Some(rd));
        assert!(clamped.issue_deadline() <= rd);
    }

    #[test]
    fn opencode_zero_minutes_disables_the_per_issue_cap() {
        let uncapped =
            OpenCodeAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(0);
        let capped =
            OpenCodeAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(1000);
        assert!(uncapped.issue_deadline() > capped.issue_deadline());

        let rd = Instant::now() + Duration::from_secs(1);
        let bounded = OpenCodeAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(0)
            .with_run_deadline(Some(rd));
        assert!(bounded.issue_deadline() <= rd);
    }

    // ── trait binding (compile-level) ─────────────────────────────────────────

    #[test]
    fn opencode_agent_is_a_dyn_agent() {
        // Proves `OpenCodeAgent: Agent` and that it can be handed to the core as a
        // `&dyn Agent` (the core never learns the vendor).
        let agent = OpenCodeAgent::new(None, PathBuf::from("/run")).with_variant(None);
        let _as_dyn: &dyn Agent = &agent;
    }

    // ── prompt asset ─────────────────────────────────────────────────────────

    #[test]
    fn plan_charter_file_carries_full_prompt() {
        // The full charter lands on disk (mirrors exec.md) and per-issue stdin
        // stays a one-line pointer — pins the byte reduction issue #80 delivers.
        let base =
            std::env::temp_dir().join(format!("ralphy-opencode-charter-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);
        fs::create_dir_all(ws.ralphy_dir()).unwrap();

        fs::write(ws.plan_charter_path(), PROMPT_PLAN_OPENCODE).unwrap();
        assert_eq!(
            fs::read_to_string(ws.plan_charter_path()).unwrap(),
            PROMPT_PLAN_OPENCODE
        );
        assert!(ralphy_adapter_support::PLAN_CHARTER.len() * 50 < PROMPT_PLAN_OPENCODE.len());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn prompt_plan_opencode_has_no_execution_model_line() {
        assert!(
            !PROMPT_PLAN_OPENCODE.contains("## Execution model"),
            "the OpenCode plan prompt must drop the complexity tier line (D3/D8a)"
        );
    }

    #[test]
    fn prompt_plan_opencode_keeps_reviewer_step() {
        assert!(
            PROMPT_PLAN_OPENCODE.contains("reviewer"),
            "planning prompt must reference the reviewer skill"
        );
        let lower = PROMPT_PLAN_OPENCODE.to_lowercase();
        assert!(
            lower.contains("only") && lower.contains("commits you made"),
            "reviewer step must scope to this issue's own commits"
        );
        // No Claude Task-tool idiom carried over.
        assert!(
            !PROMPT_PLAN_OPENCODE.contains("independent subagent"),
            "must not use Claude 'independent subagent' phrasing"
        );
        // The reviewer step commits to the concrete working mechanism: the
        // inline `reviewer` skill, not a subagent (opencode#29616/#20059 block
        // headless custom-subagent dispatch — see ADR-0005 D8).
        assert!(
            lower.contains("inline") && lower.contains("skill"),
            "reviewer step must name the inline reviewer skill mechanism"
        );
        // No subagent-dispatch phrasing for the reviewer: the prompt must not
        // claim the reviewer runs "as a subagent".
        assert!(
            !lower.contains("as a subagent"),
            "reviewer step must not describe the reviewer as running as a subagent"
        );
    }

    #[test]
    fn prompt_plan_opencode_carries_finalize_trailer() {
        // Pin the FULL literal (suffix + spacing), not just the prefix: a drift to
        // `issue = <N> -->` would keep a prefix check green yet make the trailer no
        // longer match `plan_is_finalized_for`, silently disabling resume.
        assert!(
            PROMPT_PLAN_OPENCODE.contains("<!-- ralphy-plan: issue=<N> -->"),
            "planning prompt must instruct writing the exact finalized-plan trailer"
        );
    }
}
