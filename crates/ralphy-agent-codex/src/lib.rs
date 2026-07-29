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
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use ralphy_adapter_support::{
    list_session_files, run_exec_session, run_plan_session, ExecCfg, IssueBudget, PlanCfg,
    PROMPT_EXECUTE,
};
use ralphy_core::{git, plan, Agent, Execution, Issue, Plan, PlanLimit, Workspace};
use tracing::info;

mod auth;
mod command;
mod outcome;
mod skills;
mod tasks;
mod usage;
/// `codex exec -i <FILE>` attaches images to the initial prompt, so the triage
/// session delivers fetched screenshots by argv path (ADR-0025 §4).
pub const ACCEPTS_IMAGES: bool = true;

use auth::{
    is_codex_auth_error, is_codex_limit_text, parse_codex_reset_hint, CODEX_AUTH_ERROR_MSG,
};
use command::{
    build_codex_command, codex_config_model, recommended_tier, tier_to_model_effort,
    CODEX_MODEL_SOL, DEFAULT_CODEX_EFFORT,
};
use outcome::classify_codex_outcome;
use skills::materialize_codex_skills;
pub use tasks::{consolidate_knowledge, diagnose_repo, draft_issues, triage_issues};
use usage::{
    codex_sessions_dir, fold_execute_rollout_usage, fold_plan_rollout_usage, rollout_session_id,
};

/// The Codex planning prompt, embedded so the binary is self-contained as a global
/// tool. A variant of `prompt.plan.md` that emits a vendor-neutral
/// `low|medium|high|xhigh` complexity tier (routed to the executor model+effort,
/// ADR-0004 Amendments 2026-07-10 and 2026-07-24) instead of a Claude model name.
/// Copied to
/// `.ralphy/plan-charter.md` for the live session to read; only a one-line
/// pointer is piped on stdin. Single source of truth lives at `assets/prompts/`.
const PROMPT_PLAN_CODEX: &str = include_str!("../../../assets/prompts/prompt.plan.codex.md");

/// Drives the `codex` CLI. `model` is the operator override (else the user's
/// Codex config, else the tier-routed family table); `plan_effort`/`exec_effort`
/// are the operator's resolved Effort words (default `medium` when unset);
/// `run_dir` is where the captured logs live; `max_minutes_per_issue` is the
/// per-issue wall budget, clamped to `run_deadline` when the run carries a
/// global deadline.
pub struct CodexAgent {
    model: Option<String>,
    plan_effort: Option<String>,
    exec_effort: Option<String>,
    run_dir: PathBuf,
    budget: IssueBudget,
    #[cfg(test)]
    run_hook: Option<
        std::sync::Arc<dyn Fn() -> Result<ralphy_adapter_support::HeadlessRun> + Send + Sync>,
    >,
}

impl CodexAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            model,
            plan_effort: None,
            exec_effort: None,
            run_dir,
            budget: IssueBudget::new(ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE),
            #[cfg(test)]
            run_hook: None,
        }
    }

    #[cfg(test)]
    fn with_run_hook(
        mut self,
        hook: impl Fn() -> Result<ralphy_adapter_support::HeadlessRun> + Send + Sync + 'static,
    ) -> Self {
        self.run_hook = Some(std::sync::Arc::new(hook));
        self
    }

    /// Set the planning-phase reasoning effort (`model_reasoning_effort`).
    /// `None` keeps the vendor default ([`DEFAULT_CODEX_EFFORT`]).
    pub fn with_plan_effort(mut self, effort: Option<String>) -> Self {
        self.plan_effort = effort;
        self
    }

    /// Set the execution-phase reasoning effort (`model_reasoning_effort`).
    /// `None` keeps the vendor default ([`DEFAULT_CODEX_EFFORT`]).
    pub fn with_exec_effort(mut self, effort: Option<String>) -> Self {
        self.exec_effort = effort;
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
    /// `ClaudeAgent::with_run_deadline`).
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

    /// The single model decision point, in precedence order: the explicit
    /// `--exec-model` override, then the `model` from the user's Codex config, then
    /// `routed` — the role's row in the family table (planning → Sol; execution →
    /// the plan tier via `tier_to_model_effort`). Honouring the config keeps a
    /// subscription account on the model it is entitled to with no explicit flag;
    /// the routed fallback replaces the dead `gpt-5-codex` floor (ADR-0004,
    /// Amendment 2026-07-10).
    fn resolve_model(&self, routed: &str) -> String {
        if let Some(m) = self.model.as_deref() {
            return m.to_string();
        }
        codex_config_model().unwrap_or_else(|| routed.to_string())
    }

    /// Planning-phase effort for argv/emit: operator flag, else vendor default.
    fn resolved_plan_effort(&self) -> &str {
        self.plan_effort.as_deref().unwrap_or(DEFAULT_CODEX_EFFORT)
    }

    /// Execution-phase effort for argv/emit: the operator's `--exec-effort` flag
    /// wins on every issue; when unset it falls back to `tier_default` — the
    /// effort the plan's complexity rung routed to (ADR-0004, Amendment
    /// 2026-07-24), not a flat `medium`.
    fn resolved_exec_effort<'a>(&'a self, tier_default: &'a str) -> &'a str {
        self.exec_effort.as_deref().unwrap_or(tier_default)
    }
}

impl Agent for CodexAgent {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        let plan_path = ws.plan_path();
        // Planning routes to the flagship model — the highest-leverage step of
        // the run (ADR-0004, Amendment 2026-07-10; the `opus`-planning analogue).
        let model = self.resolve_model(CODEX_MODEL_SOL);
        let out_path = ws.ralphy_dir().join("codex-last.txt");
        let log_path = self.run_dir.join("codex.log");
        // Snapshotting the rollout tree around the call is Codex-specific (ADR-0008
        // D10, appeared-over-grew): a file that APPEARED is this run's session, one
        // that merely grew is a concurrent pre-existing session and is excluded.
        let sessions_dir = codex_sessions_dir();
        let snapshot = || {
            sessions_dir
                .as_deref()
                .map(|d| list_session_files(d, "jsonl", true, Some("rollout-")))
                .unwrap_or_default()
        };

        let run = || {
            materialize_codex_skills(ws)?;
            let _ = fs::remove_file(&out_path);
            let before = snapshot();
            let effort = self.resolved_plan_effort();
            let cmd = build_codex_command(&model, effort, ws.repo_root(), &out_path);
            ralphy_core::emit::planning("codex exec", &model, effort, "");
            // Clock the budget at the spawn, not method entry, so the run_deadline
            // clamp isn't eroded by the preceding dir/snapshot setup.
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            let r = self.run_codex(cmd, ralphy_adapter_support::PLAN_CHARTER, timeout)?;
            let after = snapshot();
            Ok((r, (before, after)))
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
                charter_body: PROMPT_PLAN_CODEX,
                log_path: &log_path,
                auth_msg: CODEX_AUTH_ERROR_MSG,
                no_plan_msg: "codex produced no plan",
            },
            run,
            is_codex_auth_error,
            // A usage limit during planning is not a generic failure: surface it as
            // a typed `PlanLimit` (with the parsed reset hint) so the runner routes
            // it through the same stop-and-report / auto-resume path as an
            // execute-time `Outcome::Limit`, rather than aborting the whole run.
            |log| {
                ralphy_adapter_support::detect_limit(
                    log,
                    is_codex_limit_text,
                    parse_codex_reset_hint,
                )
                .map(|reset| PlanLimit { reset }.into())
            },
        )?;

        // None = resumed (finalized plan kept, no vendor run): no rollout payload to
        // fold, so report zero planning tokens — the whole point of the resume fix.
        let (usage, session_id) = match session {
            Some((_, (before, after))) => (
                fold_plan_rollout_usage(&before, &after, &model),
                rollout_session_id(&before, &after),
            ),
            None => (ralphy_core::Usage::default(), None),
        };
        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            recommended_model: recommended_tier(&md),
            path: plan_path,
            usage,
            session_id,
        })
    }

    fn execute(&self, plan: &Plan, ws: &Workspace) -> Result<Execution> {
        // Execution routes the plan's neutral complexity tier to one rung on the
        // (model, effort) ladder — low→Luna:low, medium→Terra:medium,
        // high→Sol:medium, xhigh→Sol:high (ADR-0004 Amendment 2026-07-24). The
        // tier's effort is the DEFAULT; an explicit `--exec-effort` still wins.
        let (routed_model, routed_effort) = tier_to_model_effort(plan.recommended_model.as_deref());
        let model = self.resolve_model(routed_model);
        let effort = self.resolved_exec_effort(routed_effort);
        let out_path = ws.ralphy_dir().join("codex-last.txt");
        let log_path = self.run_dir.join("codex.log");
        // HEAD before/after bounds the work this call committed (progress guard).
        let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        // Snapshot the rollout tree around the call for appeared-over-grew token
        // capture (ADR-0008 D10), the same rule the Claude adapter uses.
        let sessions_dir = codex_sessions_dir();
        let snapshot = || {
            sessions_dir
                .as_deref()
                .map(|d| list_session_files(d, "jsonl", true, Some("rollout-")))
                .unwrap_or_default()
        };

        let run = || {
            materialize_codex_skills(ws)?;
            let _ = fs::remove_file(&out_path);
            let before = snapshot();
            let cmd = build_codex_command(&model, effort, ws.repo_root(), &out_path);
            ralphy_core::emit::executing("codex exec", 0, &model, effort, "");
            // Clock the budget at the spawn, not method entry, so the run_deadline
            // clamp isn't eroded by the preceding dir/snapshot setup.
            let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
            let r = self.run_codex(cmd, PROMPT_EXECUTE, timeout)?;
            let after = snapshot();
            Ok((r, (before, after)))
        };

        let ralphy_dir = ws.ralphy_dir();
        let (r, (before, after)) = run_exec_session(
            ExecCfg {
                ralphy_dir: &ralphy_dir,
                run_dir: &self.run_dir,
                log_path: &log_path,
                auth_msg: CODEX_AUTH_ERROR_MSG,
            },
            run,
            is_codex_auth_error,
        )?;

        let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
        let committed = before_sha != after_sha;
        let out = fs::read_to_string(&out_path).unwrap_or_default();

        let outcome =
            classify_codex_outcome(r.exited_cleanly, r.timed_out, committed, &out, &r.log);
        info!(
            ?outcome,
            exited_cleanly = r.exited_cleanly,
            timed_out = r.timed_out,
            committed,
            "codex execution ended"
        );
        Ok(Execution {
            outcome,
            usage: fold_execute_rollout_usage(&before, &after, &model),
            session_id: rollout_session_id(&before, &after),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralphy_adapter_support::HeadlessRun;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clean_run() -> HeadlessRun {
        HeadlessRun {
            stdout: String::new(),
            log: String::new(),
            exited_cleanly: true,
            timed_out: false,
            idle_killed: false,
            stopped: false,
            exit_code: Some(0),
        }
    }

    fn rollout(model_tokens: u64) -> String {
        format!(
            "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":{model_tokens},\"cached_input_tokens\":0,\"output_tokens\":0}}}}}}}}"
        )
    }

    fn serialized_ledger_model(usage: &ralphy_core::Usage) -> String {
        let record = ralphy_core::ledger::LedgerRecord {
            project: "owner/repo".into(),
            actor_email: "dev@example.com".into(),
            actor_name: "Dev".into(),
            ralphy_version: "test".into(),
            issue: 356,
            phase: "test".into(),
            agent: "codex".into(),
            model: usage.model.clone().unwrap_or_else(|| "unknown".into()),
            session_id: Some("session".into()),
            outcome: "done".into(),
            tokens: usage.clone(),
            ts: "2026-07-29T00:00:00Z".into(),
        };
        let value: serde_json::Value =
            serde_json::from_str(&ralphy_core::ledger::record_line(&record).unwrap()).unwrap();
        value["model"].as_str().unwrap().to_string()
    }

    #[test]
    fn plan_and_execute_agent_paths_serialize_the_resolved_model() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let restore = std::env::var_os("CODEX_HOME");
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let codex_home = temp.path().join("codex");
        let sessions = codex_home.join("sessions");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&sessions).unwrap();
        std::env::set_var("CODEX_HOME", &codex_home);
        let ws = Workspace::new(&repo);
        let plan_path = ws.plan_path();
        let plan_rollout = sessions.join("rollout-plan.jsonl");
        let plan_agent = CodexAgent::new(Some("gpt-5-codex".into()), temp.path().join("plan-run"))
            .with_run_hook(move || {
                std::fs::write(
                    &plan_path,
                    "# Plan\n## Execution model: high\n## Steps\n- [ ] implement\n",
                )?;
                std::fs::write(&plan_rollout, rollout(10))?;
                Ok(clean_run())
            });
        let issue = Issue {
            number: 356,
            title: "model recovery".into(),
            body: String::new(),
            labels: Vec::new(),
            comments: Vec::new(),
        };

        let plan = Agent::plan(&plan_agent, &issue, &ws).unwrap();
        assert_eq!(plan.usage.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(serialized_ledger_model(&plan.usage), "gpt-5-codex");

        let execute_rollout = sessions.join("rollout-execute.jsonl");
        let out_path = ws.ralphy_dir().join("codex-last.txt");
        let execute_agent =
            CodexAgent::new(Some("gpt-5-codex".into()), temp.path().join("execute-run"))
                .with_run_hook(move || {
                    std::fs::write(&execute_rollout, rollout(20))?;
                    std::fs::write(&out_path, "RALPHY_DONE_EXIT\n")?;
                    Ok(clean_run())
                });

        let execution = Agent::execute(&execute_agent, &plan, &ws).unwrap();
        assert_eq!(execution.usage.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(serialized_ledger_model(&execution.usage), "gpt-5-codex");

        match restore {
            Some(value) => std::env::set_var("CODEX_HOME", value),
            None => std::env::remove_var("CODEX_HOME"),
        }
    }

    /// The log shape that threw away a finished #356 execute: a repo grep echoed
    /// this adapter's own `auth.rs` — [`CODEX_AUTH_ERROR_MSG`] and its literal
    /// `401 Unauthorized` — into the child's combined stdout+stderr.
    fn log_echoing_this_crates_auth_source() -> String {
        format!(
            "crates/ralphy-agent-codex/src\\auth.rs-6-pub(crate) const CODEX_AUTH_ERROR_MSG: &str =\n\
             crates/ralphy-agent-codex/src\\auth.rs-7-    \"{CODEX_AUTH_ERROR_MSG}\";\n"
        )
    }

    fn done_plan(ws: &Workspace) -> Plan {
        Plan {
            path: ws.plan_path(),
            open_steps: 1,
            recommended_model: Some("high".into()),
            usage: ralphy_core::Usage::default(),
            session_id: None,
        }
    }

    /// A green execution must survive the agent reading source that documents the
    /// auth banner — and a genuinely failed one must still say `codex login`.
    #[test]
    fn a_clean_execute_outlives_its_own_auth_message_in_the_log() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let restore = std::env::var_os("CODEX_HOME");
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let codex_home = temp.path().join("codex");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(codex_home.join("sessions")).unwrap();
        std::env::set_var("CODEX_HOME", &codex_home);
        let ws = Workspace::new(&repo);
        let log = log_echoing_this_crates_auth_source();
        // The detector DOES fire on this text: what protects the run is the
        // failed-run gate in `run_exec_session`, not the needle.
        assert!(is_codex_auth_error(&log));

        let sentinel = ws.ralphy_dir().join("codex-last.txt");
        let clean_log = log.clone();
        let green = CodexAgent::new(Some("gpt-5-codex".into()), temp.path().join("green-run"))
            .with_run_hook(move || {
                std::fs::write(&sentinel, "RALPHY_DONE_EXIT\n")?;
                Ok(HeadlessRun {
                    log: clean_log.clone(),
                    ..clean_run()
                })
            });
        let execution = Agent::execute(&green, &done_plan(&ws), &ws)
            .expect("a clean run that merely read auth.rs is not an auth failure");
        assert_eq!(execution.outcome, ralphy_core::Outcome::Done);

        // Negative control: the same text from a run the CLI failed is the real
        // signal, and still bails with the actionable message.
        let signed_out = CodexAgent::new(
            Some("gpt-5-codex".into()),
            temp.path().join("signed-out-run"),
        )
        .with_run_hook(move || {
            Ok(HeadlessRun {
                log: log.clone(),
                exited_cleanly: false,
                exit_code: Some(1),
                ..clean_run()
            })
        });
        let err = Agent::execute(&signed_out, &done_plan(&ws), &ws).unwrap_err();
        assert!(err.to_string().contains(CODEX_AUTH_ERROR_MSG), "got: {err}");

        match restore {
            Some(value) => std::env::set_var("CODEX_HOME", value),
            None => std::env::remove_var("CODEX_HOME"),
        }
    }

    // ── with_max_minutes_per_issue ──────────────────────────────────────────

    #[test]
    fn codex_honours_max_minutes_per_issue() {
        assert_eq!(
            CodexAgent::new(None, PathBuf::from("/run"))
                .budget
                .max_minutes_per_issue,
            ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE
        );
        let a = CodexAgent::new(None, PathBuf::from("/run")).with_max_minutes_per_issue(120);
        assert_eq!(a.budget.max_minutes_per_issue, 120);
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

    // ── effort → model_reasoning_effort ─────────────────────────────────────

    #[test]
    fn exec_effort_override_wins_over_the_tier_default() {
        // An explicit `--exec-effort high` beats the tier's routed default on
        // every issue — even when the tier (here `medium`→Terra:medium) would
        // otherwise route to `medium`.
        let agent =
            CodexAgent::new(None, PathBuf::from("/run")).with_exec_effort(Some("high".into()));
        assert_eq!(agent.resolved_exec_effort("medium"), "high");
        let cmd = build_codex_command(
            CODEX_MODEL_SOL,
            agent.resolved_exec_effort("medium"),
            std::path::Path::new("/repo"),
            std::path::Path::new("/repo/out.txt"),
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|a| a == "model_reasoning_effort=\"high\""),
            "argv must carry the operator's high effort: {args:?}"
        );
    }

    #[test]
    fn unset_exec_effort_falls_back_to_the_tier_default() {
        // With no `--exec-effort`, the effort is whatever the tier routed — no
        // longer a flat `medium`. `xhigh`→Sol:high proves the high rung reaches
        // the argv unaided by an operator flag.
        let agent = CodexAgent::new(None, PathBuf::from("/run"));
        let (model, tier_effort) = command::tier_to_model_effort(Some("xhigh"));
        assert_eq!(agent.resolved_exec_effort(tier_effort), "high");
        let cmd = build_codex_command(
            model,
            agent.resolved_exec_effort(tier_effort),
            std::path::Path::new("/repo"),
            std::path::Path::new("/repo/out.txt"),
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|a| a == "model_reasoning_effort=\"high\""),
            "unset effort must inherit the tier's routed effort: {args:?}"
        );
    }

    #[test]
    fn plan_and_execute_use_the_resolved_effort_helpers() {
        // Pins the production call sites to the same helpers the argv tests drive —
        // a plan/execute that ignores stored fields would otherwise stay green.
        let prod = include_str!("lib.rs");
        assert!(
            prod.contains("let effort = self.resolved_plan_effort();"),
            "plan must bind effort via resolved_plan_effort"
        );
        assert!(
            prod.contains("let effort = self.resolved_exec_effort(routed_effort);"),
            "execute must bind effort via resolved_exec_effort(routed_effort)"
        );
    }

    #[test]
    fn effort_does_not_alter_the_tier_routed_model() {
        assert_eq!(
            tier_to_model_effort(Some("low")).0,
            command::CODEX_MODEL_LUNA
        );
        assert_eq!(
            tier_to_model_effort(Some("medium")).0,
            command::CODEX_MODEL_TERRA
        );
        assert_eq!(tier_to_model_effort(Some("high")).0, CODEX_MODEL_SOL);
        assert_eq!(tier_to_model_effort(Some("xhigh")).0, CODEX_MODEL_SOL);

        // A fixed model id in `-m` is unchanged when only the effort argv varies —
        // effort couples to the tier as a DEFAULT, but the `-m` column is set by
        // the model, never by the effort word.
        for effort in ["low", "high"] {
            let cmd = build_codex_command(
                command::CODEX_MODEL_TERRA,
                effort,
                std::path::Path::new("/repo"),
                std::path::Path::new("/repo/out.txt"),
            );
            let args: Vec<String> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let m = args.iter().position(|a| a == "-m").expect("-m present");
            assert_eq!(
                args[m + 1],
                command::CODEX_MODEL_TERRA,
                "effort={effort} must not alter -m: {args:?}"
            );
        }
    }

    // ── resolve_model ───────────────────────────────────────────────────────

    #[test]
    fn resolve_model_override_wins() {
        // The explicit --exec-model override wins over config and the tier-routed
        // fallback, with no dependence on the machine's Codex config.
        let overridden = CodexAgent::new(Some("gpt-5".into()), PathBuf::from("/run"));
        assert_eq!(overridden.resolve_model(CODEX_MODEL_SOL), "gpt-5");
        assert_eq!(
            overridden.resolve_model(command::tier_to_model_effort(Some("low")).0),
            "gpt-5"
        );
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

    #[test]
    fn prompt_plan_codex_carries_finalize_trailer() {
        // Pin the FULL literal (suffix + spacing), not just the prefix: a drift to
        // `issue = <N> -->` would keep a prefix check green yet make the trailer no
        // longer match `plan_is_finalized_for`, silently disabling resume.
        assert!(
            PROMPT_PLAN_CODEX.contains("<!-- ralphy-plan: issue=<N> -->"),
            "planning prompt must instruct writing the exact finalized-plan trailer"
        );
    }
}
