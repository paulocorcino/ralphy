//! Headless execution: the `claude -p` loop (`execute_headless`) and its
//! completion state machine — per-call classification (`classify_exec_call`),
//! the loop transition (`headless_step`), and the terminal-reason → core
//! [`Outcome`] mappings. `classify_outcome` (the live-PTY end-state classifier)
//! also lives here so both execution paths share one outcome vocabulary.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ralphy_adapter_support::{
    classify, failed_run_stop, CompletionSignals, HeadlessCall, RunFacts, RunStop, PROMPT_EXECUTE,
};
use ralphy_core::{git, plan, Outcome, Plan, ResetAt, Workspace};
use tracing::info;

use crate::auth::{
    claude_reset, is_claude_auth_error, is_limit_text, transcript_limit, CLAUDE_AUTH_ERROR_MSG,
};
use crate::plan::materialize_plugin;
use crate::settings::SettingsFile;
use crate::stream::{stream_text, user_message};
use crate::ClaudeAgent;

pub(crate) fn headless_args(
    settings: &Path,
    plugin_dir: &Path,
    model: &str,
    effort: Option<&str>,
) -> Vec<String> {
    let mut args = vec!["-p".into(), "--dangerously-skip-permissions".into()];
    args.extend(crate::STREAM_JSON_ARGS.iter().map(ToString::to_string));
    args.extend([
        "--settings".into(),
        settings.to_string_lossy().into_owned(),
        "--plugin-dir".into(),
        plugin_dir.to_string_lossy().into_owned(),
        "--model".into(),
        model.into(),
    ]);
    args.extend(crate::effort_args(effort));
    args
}

impl ClaudeAgent {
    /// Spawn a single `claude -p` call for headless execution, writing
    /// `PROMPT_EXECUTE` as one stream-json user message on stdin once the
    /// session's `SessionStart` line proves its hooks run, and draining stdout/stderr
    /// via reader threads to avoid pipe-buffer deadlock. Polls `try_wait` until
    /// `timeout` fires; kills the child on expiry and returns `exited = false`.
    #[expect(
        clippy::too_many_arguments,
        reason = "one call needs its directories, model, effort, timeout and call number"
    )]
    fn run_headless_call(
        &self,
        cmd_dir: &Path,
        settings: &SettingsFile,
        plugin_dir: &Path,
        model: &str,
        effort: Option<&str>,
        timeout: Duration,
        call_index: u32,
    ) -> Result<(bool, String)> {
        let args = headless_args(&settings.path, plugin_dir, model, effort);

        let mut cmd = Command::new(crate::interactive::resolve_claude_binary());
        cmd.args(&args)
            .current_dir(cmd_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The agent-state tail (ADR-0059 §3), per call: the hooks append, the
        // watcher folds and emits until the call returns.
        let status = crate::status::Watcher::start(&self.run_dir)?;
        cmd.env(crate::status::STATUS_ENV, status.path());
        // A shell form is refused here, before the spawn (ADR-0072 D6).
        let gate = crate::status::proof_gate(
            settings,
            status.path().to_path_buf(),
            cmd_dir,
            self.exec.run_without_guard_proof,
        )?;

        // Delegate the OS-level spawn/drain/poll/kill/collect and log-persist
        // plumbing to the shared runner. Claude's `exited` ("the child exited
        // rather than being killed on the wall timeout") is `!timed_out` — NOT the
        // runner's `exited_cleanly` (a *successful* exit); the D10 auth bail stays
        // inline here.
        let log_path = self.run_dir.join(format!("exec-{call_index}.out"));
        let prompt = user_message(PROMPT_EXECUTE);
        let r = HeadlessCall::new(cmd, &prompt, timeout, &log_path)
            .hold_prompt(gate)
            .idle_minutes(self.exec.idle_minutes_for(false))
            .degraded_line(is_headless_api_degraded)
            .run()
            .context("failed to spawn the `claude` CLI for headless exec")?;
        status.stop();

        // The detectors read the text of the stream; the raw log stays on
        // disk as it came.
        let text = stream_text(&r.log);
        if is_exec_auth_failure(RunFacts::from(&r), &text) {
            bail!("{} (see {})", CLAUDE_AUTH_ERROR_MSG, log_path.display());
        }

        Ok((!r.timed_out, text))
    }

    /// Drive the issue with a `claude -p` loop (headless mode). Mirrors the
    /// `Invoke-ExecLoop` ps1 oracle: writes `exec.md`, loops up to
    /// `max_exec_calls` calls, and classifies the per-call output into a core
    /// `Outcome` via `headless_reason_to_outcome`.
    pub(crate) fn execute_headless(&self, plan: &Plan, ws: &Workspace) -> Result<Outcome> {
        std::fs::create_dir_all(&self.run_dir).context("creating the run directory")?;
        std::fs::create_dir_all(ws.ralphy_dir()).context("creating the .ralphy directory")?;

        std::fs::write(ws.ralphy_dir().join("exec.md"), PROMPT_EXECUTE)
            .context("writing .ralphy/exec.md")?;

        let settings = self.write_exec_settings()?;
        let plugin_dir = materialize_plugin(ws)?;
        let exec_model = self.resolve_exec_model(plan);
        // Effort follows the model's precedence: operator flag wins, else the
        // plan's `opus-high` rung raises it to high (ADR-0002, Amendment 2026-07-24).
        let exec_effort = self.resolve_exec_effort(plan);
        let deadline = self.issue_deadline();

        // budget_min field consumed by the telegram notifier / presenter — keep stable
        ralphy_core::emit::executing(
            &format!(
                "headless claude -p loop --max-calls {}",
                self.exec.max_exec_calls
            ),
            self.exec.max_minutes_per_issue,
            &exec_model,
            exec_effort.as_deref().unwrap_or(""),
            "",
        );

        let mut no_commit_streak = 0u32;

        for i in 1..=self.exec.max_exec_calls {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining <= Duration::from_secs(5) {
                info!(
                    call = i,
                    "per-issue deadline reached before next headless call"
                );
                return Ok(headless_reason_to_outcome(HeadlessReason::Timeout));
            }

            let before_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
            let (exited, out) = self.run_headless_call(
                ws.repo_root(),
                &settings,
                &plugin_dir,
                &exec_model,
                exec_effort.as_deref(),
                remaining,
                i,
            )?;

            let plan_md = std::fs::read_to_string(&plan.path).unwrap_or_default();
            let open_steps = plan::count_open_steps(&plan_md);
            let after_sha = git::head_sha(ws.repo_root()).unwrap_or_default();
            let committed = before_sha != after_sha;

            let classified = classify_exec_call(&out, exited, open_steps);
            match headless_step(no_commit_streak, classified, committed) {
                LoopStep::Terminal(reason) => {
                    info!(call = i, "headless call terminal");
                    return Ok(headless_reason_to_outcome(reason));
                }
                LoopStep::Continue(streak) => {
                    no_commit_streak = streak;
                    if !committed {
                        info!(
                            call = i,
                            streak = no_commit_streak,
                            "no commit this headless call"
                        );
                    }
                }
            }
        }

        info!(
            max_calls = self.exec.max_exec_calls,
            "headless loop exhausted max calls"
        );
        Ok(headless_reason_to_outcome(HeadlessReason::MaxCalls))
    }
}

/// Whether a headless exec call failed on login: a login error in its text, from
/// a call that did not exit cleanly. A call that exited cleanly keeps its result,
/// whatever its text quotes.
fn is_exec_auth_failure(run: RunFacts, text: &str) -> bool {
    failed_run_stop(&run, text, is_claude_auth_error) == Some(RunStop::Auth)
}

/// The headless `-p` degraded-line matcher, handed to the shared runner's
/// [`degraded_line`](ralphy_adapter_support::HeadlessCall::degraded_line) seam.
/// Reuses the two distinctive substrings from the PTY banner detector
/// ([`crate::api_watch::is_api_degraded_output`]) but over plain `-p` text — no
/// TUI cursor-escape stripping, since `claude -p` writes flat lines. A miss is
/// safe (the line just counts as ordinary progress); the paired test guards that
/// healthy output does not match.
fn is_headless_api_degraded(line: &str) -> bool {
    let l = line.to_lowercase();
    l.contains("waiting for api response") || l.contains("server error mid-response")
}

/// Map the session's end state to an [`Outcome`]. Extracts the Stop-hook flag and
/// transcript usage-limit text into [`CompletionSignals`] and delegates the
/// precedence ordering to the shared [`classify`] ladder (ADR-0023 D1/D2).
pub(crate) fn classify_outcome(
    flag: Option<&str>,
    timed_out: bool,
    transcript: Option<&str>,
) -> Outcome {
    let mut done = false;
    let mut blocked = None;
    if let Some(f) = flag {
        let f = f.trim();
        if f == "DONE" {
            done = true;
        } else if let Some(reason) = f.strip_prefix("BLOCKED") {
            blocked = Some(reason.trim().to_string());
        }
    }
    classify(CompletionSignals {
        done,
        blocked,
        limit: transcript.and_then(transcript_limit),
        committed: false,
        timed_out,
        exited_ok: !timed_out,
        errored: false,
    })
}

/// Terminal reason for one headless `-p` call, mirroring `Invoke-ExecLoop`'s
/// returned strings. Mapped to a core [`Outcome`] by [`headless_reason_to_outcome`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum HeadlessReason {
    Done,
    Blocked(String),
    Limit(Option<ResetAt>),
    Timeout,
    Stuck,
    MaxCalls,
}

/// Classify the result of a single headless `-p` call. Returns the terminal
/// reason if this call ends the loop, or `None` to continue to the next call.
///
/// Extracts the done sentinel, blocked sentinel, and limit text into
/// [`CompletionSignals`] and delegates the precedence ordering to the shared
/// [`classify`] ladder (ADR-0023 D1/D2).
pub(crate) fn classify_exec_call(
    out: &str,
    exited: bool,
    open_steps: usize,
) -> Option<HeadlessReason> {
    let signals = CompletionSignals {
        done: ralphy_adapter_support::done_sentinel(out) || open_steps == 0,
        blocked: ralphy_adapter_support::blocked_reason(out),
        limit: ralphy_adapter_support::detect_limit(out, is_limit_text, claude_reset),
        committed: false,
        timed_out: !exited,
        exited_ok: exited,
        errored: false,
    };
    match classify(signals) {
        Outcome::Limit(t) => Some(HeadlessReason::Limit(t)),
        Outcome::Done => Some(HeadlessReason::Done),
        Outcome::Timeout => Some(HeadlessReason::Timeout),
        Outcome::Blocked(s) => Some(HeadlessReason::Blocked(s)),
        Outcome::Stuck => None,
    }
}

/// One transition of the headless loop's decision logic, factored out of
/// [`ClaudeAgent::execute_headless`] so the code the loop runs *is* the code the
/// tests exercise — no transcribed copy that can silently drift.
#[derive(Debug, Clone, PartialEq)]
enum LoopStep {
    /// This call ends the loop with the given reason.
    Terminal(HeadlessReason),
    /// Continue to the next call carrying this no-commit streak.
    Continue(u32),
}

/// Decide the loop's next step from a call's classification and whether it
/// committed. A terminal `classified` reason ends the loop immediately;
/// otherwise the no-commit streak advances and two consecutive no-commit calls
/// are `Stuck` (mirrors `Invoke-ExecLoop`'s `$stuck -ge 2`).
fn headless_step(streak: u32, classified: Option<HeadlessReason>, committed: bool) -> LoopStep {
    if let Some(reason) = classified {
        return LoopStep::Terminal(reason);
    }
    let streak = if committed { 0 } else { streak + 1 };
    if streak >= 2 {
        LoopStep::Terminal(HeadlessReason::Stuck)
    } else {
        LoopStep::Continue(streak)
    }
}

/// Collapse a headless terminal reason onto an existing core [`Outcome`].
/// `MaxCalls` maps to `Stuck` — it is a headless-only safety cap that does not
/// warrant a new core variant (ADR-0002).
fn headless_reason_to_outcome(r: HeadlessReason) -> Outcome {
    match r {
        HeadlessReason::Done => Outcome::Done,
        HeadlessReason::Blocked(s) => Outcome::Blocked(s),
        HeadlessReason::Limit(t) => Outcome::Limit(t),
        HeadlessReason::Timeout => Outcome::Timeout,
        HeadlessReason::Stuck | HeadlessReason::MaxCalls => Outcome::Stuck,
    }
}

#[cfg(test)]
mod tests;
