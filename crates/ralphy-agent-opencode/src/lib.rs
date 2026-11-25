//! The `OpenCode` CLI adapter: drives `opencode run` behind the core [`Agent`](ralphy_core::Agent)
//! contract. Everything OpenCode-specific — the binary, the model/variant flags,
//! the headless invocation, the line-delimited-JSON event stream, and the
//! signal→[`ralphy_core::Outcome`] mapping — is confined here. See docs/adr/0005.
//!
//! Like the Codex adapter (and unlike Claude's live PTY session), `OpenCode` needs
//! no interactive session: `plan` and `execute` both run headless `opencode run`
//! with the prompt piped on stdin, and completion is detected from the
//! `RALPHY_DONE_EXIT`/`RALPHY_BLOCKED_EXIT` sentinels parsed out of the JSON
//! `text` parts, a JSON `error` event, the process exit code, and a HEAD-diff
//! commit check — mapped onto the same core [`ralphy_core::Outcome`].
//!
//! Skills materialization (ADR-0005 D7) is implemented here: before every `plan`
//! and `execute` call the embedded skills tree is extracted to `<repo>/.ralphy/skills`
//! and the absolute path is injected as `OPENCODE_CONFIG_CONTENT` so `OpenCode`'s
//! `skills.paths` config key points at it. Auth-error (D6) is implemented:
//! `is_opencode_auth_error` detects `ProviderAuthError` in the combined log and an
//! actionable bail fires in both `plan` and `execute` before any generic
//! classification. Usage-limit (D9) is implemented: `parse_opencode_limit` scans
//! the JSON stream for a 429/`APIError` or documented rate-limit string (and
//! `parse_opencode_log_limit` the logfmt server log for quota blocks that never
//! reach the JSON stream), and `classify_opencode_outcome` upgrades
//! `Timeout`/`Stuck` to `Outcome::Limit` when one is seen. Any reset hint is then
//! **discarded** (`unschedulable_opencode_limit`): `OpenCode` fronts many providers
//! whose reset strings are unreliable (`FinCal` #73), so the runner never schedules
//! on one and instead retries on the ~30-min synthetic cadence (ADR-0030 D1).
//! Auto-resume is the default for all agents; `--stop-on-limit` is the opt-out
//! (ADR-0030 D3) — it is no longer forced for `OpenCode`.
// D7 of the lint plan: production code here parses outside data; indexing must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use ralphy_adapter_support::IssueBudget;

#[cfg(test)]
mod cleanup;
mod command;
mod events;
mod outcome;
mod phases;
mod skills;
mod status;
mod tasks;
mod usage;

/// `OpenCode`'s headless `run` has no image-input channel, so images are never
/// fetched for it — the manifest shows `not fetched (opencode has no image
/// input)` (ADR-0025 §4). A future vision `OpenCode` flips this in its own crate.
pub const ACCEPTS_IMAGES: bool = false;

use command::{build_watched_opencode_command, StatePlugin};
pub use tasks::{consolidate_knowledge, diagnose_repo, draft_issues, list_models, triage_issues};

/// The `OpenCode` planning prompt, embedded so the binary is self-contained as a
/// global tool. A variant of `prompt.plan.md` with the `## Execution model` tier
/// line removed (`OpenCode` drops complexity routing, ADR-0005 D3/D8a) and the
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
    /// `None` / empty → `OpenCode` resolves the model itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl OpenCodeSettings {
    /// The settings-file section this struct lives under.
    pub const SECTION: &'static str = "opencode";
}

/// Drives the `opencode` CLI. `model` is the operator override (omitted entirely
/// when `None`, deferring to `OpenCode`'s own resolution, ADR-0005 D4); `variant`
/// is the operator's optional provider-native dialect (`--exec-variant`), passed
/// through only when set (D3 / ADR-0044 D8) — orthogonal to Ralphy's Effort word;
/// `plan_effort`/`exec_effort` accept the neutral word at the CLI and are a
/// documented no-op here (ADR-0044 D4); `run_dir` is where the captured logs live;
/// `max_minutes_per_issue` is the per-issue wall budget, clamped to `run_deadline`
/// when the run carries a global deadline.
pub struct OpenCodeAgent {
    model: Option<String>,
    variant: Option<String>,
    plan_effort: Option<String>,
    exec_effort: Option<String>,
    run_dir: PathBuf,
    budget: IssueBudget,
    #[cfg(test)]
    run_hook: Option<
        std::sync::Arc<dyn Fn() -> Result<ralphy_adapter_support::HeadlessRun> + Send + Sync>,
    >,
}

impl OpenCodeAgent {
    pub fn new(model: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            model,
            variant: None,
            plan_effort: None,
            exec_effort: None,
            run_dir,
            budget: IssueBudget::new(ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE),
            #[cfg(test)]
            run_hook: None,
        }
    }

    /// A stand-in for the `opencode run` child, so a test drives a plan or an
    /// execute with no `OpenCode` installed.
    #[cfg(test)]
    fn with_run_hook(
        mut self,
        hook: impl Fn() -> Result<ralphy_adapter_support::HeadlessRun> + Send + Sync + 'static,
    ) -> Self {
        self.run_hook = Some(std::sync::Arc::new(hook));
        self
    }

    /// One `opencode run` call with the state plugin loaded: the plugin and
    /// the status file are written in the run dir before the child starts,
    /// and the status file is tailed while it runs (ADR-0085 D2).
    fn run_watched(
        &self,
        skills_dir: &std::path::Path,
        root: &std::path::Path,
        prompt: &str,
        kill_on_stderr_line: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Result<ralphy_adapter_support::HeadlessRun> {
        // Clock the budget at the spawn, not method entry, so the run_deadline
        // clamp isn't eroded by the preceding dir/skills setup.
        let timeout = self.budget.timeout(ralphy_core::UNBOUNDED_ISSUE_HORIZON);
        let status = status::Watcher::start(&self.run_dir)?;
        let cmd = build_watched_opencode_command(
            self.model.as_deref(),
            self.variant.as_deref(),
            root,
            Some(skills_dir),
            &StatePlugin {
                plugin: status.plugin(),
                status: status.path(),
            },
        );
        #[cfg(test)]
        let r = match &self.run_hook {
            Some(hook) => hook(),
            None => self.run_opencode(cmd, prompt, timeout, kill_on_stderr_line),
        };
        #[cfg(not(test))]
        let r = self.run_opencode(cmd, prompt, timeout, kill_on_stderr_line);
        status.stop();
        r
    }

    /// Set the operator's optional `--variant` knob (ADR-0005 D3). Provider-native
    /// dialect from `--exec-variant` only — never from resolved Effort (ADR-0044
    /// D8). Passed through to `OpenCode` only when present; omitted otherwise so the
    /// adapter never sends a value the provider rejects.
    #[must_use]
    pub fn with_variant(mut self, variant: Option<String>) -> Self {
        self.variant = variant;
        self
    }

    /// Accept the resolved planning Effort word (ADR-0044 D5). Documented no-op
    /// at the discard site in `plan` (D4) — must not alter argv or map
    /// onto `--variant` (D8).
    #[must_use]
    pub fn with_plan_effort(mut self, effort: Option<String>) -> Self {
        self.plan_effort = effort;
        self
    }

    /// Accept the resolved execution Effort word (ADR-0044 D5). Documented no-op
    /// at the discard site in `execute` (D4) — must not alter argv or map
    /// onto `--variant` (D8).
    #[must_use]
    pub fn with_exec_effort(mut self, effort: Option<String>) -> Self {
        self.exec_effort = effort;
        self
    }

    /// Set the per-issue wall-clock budget in minutes (mirrors
    /// `ClaudeAgent::with_max_minutes_per_issue`).
    #[must_use]
    pub fn with_max_minutes_per_issue(mut self, minutes: u64) -> Self {
        self.budget = self.budget.with_max_minutes_per_issue(minutes);
        self
    }

    /// Set the idle watchdog window in minutes: reap the child after that long
    /// with no output at all. `0` disables it. Unlike the per-issue cap, this
    /// keys on progress rather than elapsed time (docs/adr/0038).
    #[must_use]
    pub fn with_idle_minutes(mut self, minutes: u64) -> Self {
        self.budget = self.budget.with_idle_minutes(minutes);
        self
    }

    /// Set the run's global wall-clock deadline (from `--deadline-hours`). Each
    /// issue's budget is then clamped to it, so an issue started just under the
    /// global limit can't overrun by a whole per-issue window (mirrors
    /// `CodexAgent::with_run_deadline`).
    #[must_use]
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.budget = self.budget.with_run_deadline(run_deadline);
        self
    }
}

// By hand: the test-only `run_hook` is a closure, which has no `Debug`.
impl std::fmt::Debug for OpenCodeAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenCodeAgent")
            .field("model", &self.model)
            .field("variant", &self.variant)
            .field("run_dir", &self.run_dir)
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── prompt asset ─────────────────────────────────────────────────────────

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

    /// The per-issue setter reaches the budget, and the run deadline clamps it.
    #[test]
    fn budget_setters_reach_the_issue_deadline() {
        let run_deadline = Instant::now() + std::time::Duration::from_secs(1);
        let agent = OpenCodeAgent::new(None, PathBuf::from("/run"))
            .with_max_minutes_per_issue(120)
            .with_run_deadline(Some(run_deadline));
        assert_eq!(agent.budget.max_minutes_per_issue, 120);
        assert_eq!(
            agent.budget.deadline(ralphy_core::UNBOUNDED_ISSUE_HORIZON),
            run_deadline
        );
    }
}
