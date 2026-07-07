//! The Claude Code adapter: drives `claude` behind the core [`Agent`] contract.
//! Everything Claude-specific — the binary, the model and effort flags, the
//! settings file, the PTY, completion detection — is confined here.
//!
//! `plan` runs headless `claude -p` (prompt piped on stdin). `execute` runs a
//! *live* interactive session over [`ralphy_pty`]: it lets `claude` commit onto
//! the run branch, detects completion from a flag file its Stop hook writes, and
//! reclaims the session on a per-issue wall timeout.
//!
//! The Claude-specific responsibilities are split across sibling modules:
//! [`auth`] (auth/limit detection), [`usage`] (token capture), [`settings`]
//! (settings file + hooks + model resolution), [`plan`] (planning assets),
//! [`headless`] (the `-p` loop + outcome classification), [`interactive`] (the
//! live PTY session), and [`tasks`] (the one-shot consolidate/diagnose/draft/
//! triage sessions). This module keeps the [`ClaudeAgent`] type, its
//! constructors, and the [`Agent`] impl that delegates into those modules.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use ralphy_adapter_support::{list_session_files, session_files_appeared};
use ralphy_core::{Agent, Execution, Issue, Plan, PlanLimit, Usage, Workspace};
use tracing::info;

mod auth;
mod headless;
mod interactive;
mod plan;
mod settings;
mod tasks;
mod usage;

/// Claude Code reads image files at a local path via its Read tool, so the
/// triage session reasons over a fetched screenshot directly (ADR-0025 §4).
pub const ACCEPTS_IMAGES: bool = true;

use auth::{is_claude_auth_error, is_limit_text, parse_reset_hhmm, CLAUDE_AUTH_ERROR_MSG};
use interactive::resolve_claude_binary;
use plan::{materialize_plugin, plan_prompt_for, staged_plan_env, write_plan_charter};
use settings::{recommended_model, ExecConfig, SETTINGS_JSON};
use usage::{fold_exec_usage, parse_plan_usage, parse_transcript_usage};

pub use settings::ClaudeSettings;
pub use tasks::{consolidate_knowledge, diagnose_repo, draft_issues, triage_issues};

/// The one-line charter the interactive session is launched with; it points the
/// agent at the embedded charter and the plan, and names the exit sentinel.
pub(crate) const EXEC_CHARTER: &str = "Read .ralphy/exec.md and follow it exactly to implement .ralphy/plan.md for this issue. Emit RALPHY_DONE_EXIT when finished.";

/// Drives the `claude` CLI. `plan_model`/`plan_effort` are the planning knobs;
/// the `exec_*` fields configure the interactive execution session. `run_dir` is
/// where the settings file, the captured logs, and the per-issue flag file live.
pub struct ClaudeAgent {
    plan_model: Option<String>,
    plan_effort: Option<String>,
    run_dir: PathBuf,
    exec: ExecConfig,
}

impl ClaudeAgent {
    pub fn new(plan_model: Option<String>, plan_effort: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            plan_model,
            plan_effort,
            run_dir,
            exec: ExecConfig::default(),
        }
    }

    /// Set the execution-side configuration (the composition root supplies it
    /// from the CLI flags).
    #[allow(clippy::too_many_arguments)]
    pub fn with_exec_config(
        mut self,
        exec_model: Option<String>,
        exec_effort: Option<String>,
        default_exec_model: String,
        max_minutes_per_issue: u64,
        remote_control: bool,
        headless_exec: bool,
        max_exec_calls: u32,
    ) -> Self {
        self.exec = ExecConfig {
            exec_model,
            exec_effort,
            default_exec_model,
            max_minutes_per_issue,
            remote_control,
            headless_exec,
            max_exec_calls,
            run_deadline: None,
        };
        self
    }

    /// Set the run's global wall-clock deadline (from `--deadline-hours`). Each
    /// issue's execution budget is then clamped to it, so an issue started just
    /// under the global limit can't overrun by a whole per-issue window.
    pub fn with_run_deadline(mut self, run_deadline: Option<Instant>) -> Self {
        self.exec.run_deadline = run_deadline;
        self
    }

    /// The deadline for the current issue: the per-issue budget, clamped to the
    /// run's global deadline when one is set. A budget of `0` disables the
    /// per-issue cap — the issue is then bounded only by the run deadline (or the
    /// far-future [`ralphy_core::UNBOUNDED_ISSUE_HORIZON`] when no run deadline is
    /// set).
    pub(crate) fn issue_deadline(&self) -> Instant {
        ralphy_adapter_support::issue_deadline(
            Instant::now(),
            self.exec.max_minutes_per_issue,
            self.exec.run_deadline,
            ralphy_core::UNBOUNDED_ISSUE_HORIZON,
        )
    }
}

impl Agent for ClaudeAgent {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan> {
        fs::create_dir_all(ws.ralphy_dir()).ok();
        fs::create_dir_all(&self.run_dir).ok();

        let plan_path = ws.plan_path();
        // Plan fresh every run; never reuse a stale artifact.
        let _ = fs::remove_file(&plan_path);

        let settings_path = self.run_dir.join("ralphy.settings.json");
        fs::write(&settings_path, SETTINGS_JSON).context("writing claude settings")?;

        // Provision the reviewer/staged-plan skills the prompt depends on, scoped
        // to this run via --plugin-dir (no reliance on globally-installed skills).
        let plugin_dir = materialize_plugin(ws)?;

        let (prompt, staged) = plan_prompt_for(issue);
        write_plan_charter(ws, prompt)?;

        // `--model` first (as the ps1 oracle does), then the headless flags.
        let mut args: Vec<String> = Vec::new();
        if let Some(m) = &self.plan_model {
            args.push("--model".into());
            args.push(m.clone());
        }
        args.push("-p".into());
        args.push("--dangerously-skip-permissions".into());
        // Capture per-invocation token usage off the result event (ADR-0008 D5).
        // `stream-json` requires `--verbose` on the pinned CLI; the plan markdown
        // is still written to disk by the session, so the stdout format is free to
        // change. `parse_plan_usage` skips the non-JSON warning preamble.
        args.push("--output-format".into());
        args.push("stream-json".into());
        args.push("--verbose".into());
        args.push("--settings".into());
        args.push(settings_path.to_string_lossy().into_owned());
        args.push("--plugin-dir".into());
        args.push(plugin_dir.to_string_lossy().into_owned());
        if let Some(e) = &self.plan_effort {
            args.push("--effort".into());
            args.push(e.clone());
        }

        info!(
            model = self.plan_model.as_deref().unwrap_or(""),
            effort = self.plan_effort.as_deref().unwrap_or("medium"),
            staged,
            "planning with claude -p"
        );
        let mut cmd = Command::new(resolve_claude_binary());
        cmd.args(&args)
            .current_dir(ws.repo_root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some((key, value)) = staged_plan_env(staged) {
            cmd.env(key, value);
        }
        let mut child = cmd
            .spawn()
            .context("failed to spawn the `claude` CLI (is it installed and on PATH?)")?;

        // Pipe only the one-line pointer charter on stdin (the full charter is
        // on disk at .ralphy/plan-charter.md); dropping the handle closes it so
        // claude sees EOF.
        child
            .stdin
            .take()
            .context("claude plan child stdin was not piped")?
            .write_all(ralphy_adapter_support::PLAN_CHARTER.as_bytes())
            .context("piping the plan pointer charter to claude")?;

        let out = child.wait_with_output().context("waiting for claude")?;
        let mut log = String::from_utf8_lossy(&out.stdout).into_owned();
        log.push_str(&String::from_utf8_lossy(&out.stderr));
        let _ = fs::write(self.run_dir.join("plan.log"), &log);

        if is_claude_auth_error(&log) {
            bail!(
                "{} (see {})",
                CLAUDE_AUTH_ERROR_MSG,
                self.run_dir.join("plan.log").display()
            );
        }

        if !plan_path.exists() {
            if is_limit_text(&log) {
                return Err(PlanLimit {
                    reset: parse_reset_hhmm(&log),
                }
                .into());
            }
            bail!(
                "claude produced no plan at {} (see {})",
                plan_path.display(),
                self.run_dir.join("plan.log").display()
            );
        }
        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: ralphy_core::plan::count_open_steps(&md),
            recommended_model: recommended_model(&md),
            path: plan_path,
            usage: parse_plan_usage(&log),
        })
    }

    fn execute(&self, plan: &Plan, ws: &Workspace) -> Result<Execution> {
        // Snapshot the dashed-cwd transcript dir around the whole session so a
        // file that APPEARED is this run's transcript, while one that merely grew
        // is a concurrent pre-existing session and is excluded (ADR-0008 D10,
        // appeared-over-grew). A missing dir yields empty before/after and zero
        // usage — best-effort, never failing the run.
        let transcript_dir = self.transcript_dir(ws);
        let before = transcript_dir
            .as_deref()
            .map(|d| list_session_files(d, "jsonl", false, None))
            .unwrap_or_default();

        let outcome = self.execute_outcome(plan, ws)?;

        let after = transcript_dir
            .as_deref()
            .map(|d| list_session_files(d, "jsonl", false, None))
            .unwrap_or_default();
        let per_transcript: Vec<Usage> = session_files_appeared(&before, &after)
            .iter()
            .filter_map(|p| fs::read_to_string(p).ok())
            .map(|t| parse_transcript_usage(&t))
            .collect();
        let usage = fold_exec_usage(&per_transcript, &self.resolve_exec_model(plan));
        Ok(Execution { outcome, usage })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralphy_adapter_support::PROMPT_EXECUTE;
    use std::path::PathBuf;
    use std::time::Duration;

    /// Anti-drift: the charter this adapter launches sessions with and the
    /// embedded execution prompt must both name the shared completion sentinel;
    /// `ralphy_adapter_support::DONE_SENTINEL` is the single source of truth.
    #[test]
    fn charter_and_prompt_name_the_done_sentinel() {
        assert!(EXEC_CHARTER.contains(ralphy_adapter_support::DONE_SENTINEL));
        assert!(PROMPT_EXECUTE.contains(ralphy_adapter_support::DONE_SENTINEL));
    }

    fn agent_with_minutes(minutes: u64) -> ClaudeAgent {
        ClaudeAgent::new(None, None, PathBuf::from("/run")).with_exec_config(
            None,
            Some("medium".into()),
            "sonnet".into(),
            minutes,
            true,
            false,
            6,
        )
    }

    #[test]
    fn issue_deadline_zero_minutes_disables_the_cap() {
        // `0` → no per-issue cap: the deadline sits past any finite budget…
        let uncapped = agent_with_minutes(0);
        let capped = agent_with_minutes(1000);
        assert!(uncapped.issue_deadline() > capped.issue_deadline());

        // …yet the run deadline still bounds an uncapped issue when present.
        let rd = Instant::now() + Duration::from_secs(1);
        let bounded = agent_with_minutes(0).with_run_deadline(Some(rd));
        assert!(bounded.issue_deadline() <= rd);
    }
}
