//! Headless execution: the `claude -p` loop (`execute_headless`) and its
//! completion state machine — per-call classification (`classify_exec_call`),
//! the loop transition (`headless_step`), and the terminal-reason → core
//! [`Outcome`] mappings. `classify_outcome` (the live-PTY end-state classifier)
//! also lives here so both execution paths share one outcome vocabulary.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ralphy_adapter_support::{classify, CompletionSignals, HeadlessCall, PROMPT_EXECUTE};
use ralphy_core::{git, plan, Outcome, Plan, Workspace};
use tracing::info;

use crate::auth::{
    is_claude_auth_error, is_limit_text, parse_reset_hhmm, transcript_limit, CLAUDE_AUTH_ERROR_MSG,
};
use crate::plan::materialize_plugin;
use crate::ClaudeAgent;

impl ClaudeAgent {
    /// Spawn a single `claude -p` call for headless execution, piping
    /// `PROMPT_EXECUTE` on stdin and draining stdout/stderr via reader threads
    /// to avoid pipe-buffer deadlock. Polls `try_wait` until `timeout` fires;
    /// kills the child on expiry and returns `exited = false`.
    fn run_headless_call(
        &self,
        cmd_dir: &Path,
        settings: &Path,
        plugin_dir: &Path,
        model: &str,
        timeout: Duration,
        call_index: u32,
    ) -> Result<(bool, String)> {
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--dangerously-skip-permissions".into(),
            "--settings".into(),
            settings.to_string_lossy().into_owned(),
            "--plugin-dir".into(),
            plugin_dir.to_string_lossy().into_owned(),
            "--model".into(),
            model.into(),
        ];
        if let Some(e) = &self.exec.exec_effort {
            args.push("--effort".into());
            args.push(e.clone());
        }

        let mut cmd = Command::new(crate::interactive::resolve_claude_binary());
        cmd.args(&args)
            .current_dir(cmd_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Delegate the OS-level spawn/drain/poll/kill/collect and log-persist
        // plumbing to the shared runner. Claude's `exited` ("the child exited
        // rather than being killed on the wall timeout") is `!timed_out` — NOT the
        // runner's `exited_cleanly` (a *successful* exit); the D10 auth bail stays
        // inline here.
        let log_path = self.run_dir.join(format!("exec-{}.out", call_index));
        let r = HeadlessCall::new(cmd, PROMPT_EXECUTE, timeout, &log_path)
            .idle_minutes(self.exec.idle_minutes_for(false))
            .degraded_line(is_headless_api_degraded)
            .run()
            .context("failed to spawn the `claude` CLI for headless exec")?;

        if is_claude_auth_error(&r.log) {
            bail!("{} (see {})", CLAUDE_AUTH_ERROR_MSG, log_path.display());
        }

        Ok((!r.timed_out, r.log))
    }

    /// Drive the issue with a `claude -p` loop (headless mode). Mirrors the
    /// `Invoke-ExecLoop` ps1 oracle: writes `exec.md`, loops up to
    /// `max_exec_calls` calls, and classifies the per-call output into a core
    /// `Outcome` via `headless_reason_to_outcome`.
    pub(crate) fn execute_headless(&self, plan: &Plan, ws: &Workspace) -> Result<Outcome> {
        std::fs::create_dir_all(&self.run_dir).ok();
        std::fs::create_dir_all(ws.ralphy_dir()).ok();

        std::fs::write(ws.ralphy_dir().join("exec.md"), PROMPT_EXECUTE)
            .context("writing .ralphy/exec.md")?;

        let settings_path = self.write_exec_settings()?;
        let plugin_dir = materialize_plugin(ws)?;
        let exec_model = self.resolve_exec_model(plan);
        let deadline = self.issue_deadline();

        // budget_min field consumed by the telegram notifier / presenter — keep stable
        ralphy_core::emit::executing(
            &format!(
                "headless claude -p loop --max-calls {}",
                self.exec.max_exec_calls
            ),
            self.exec.max_minutes_per_issue,
            &exec_model,
            self.exec.exec_effort.as_deref().unwrap_or("medium"),
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
                &settings_path,
                &plugin_dir,
                &exec_model,
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
    Limit(Option<String>),
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
fn classify_exec_call(out: &str, exited: bool, open_steps: usize) -> Option<HeadlessReason> {
    let signals = CompletionSignals {
        done: ralphy_adapter_support::done_sentinel(out) || open_steps == 0,
        blocked: ralphy_adapter_support::blocked_reason(out),
        limit: ralphy_adapter_support::detect_limit(out, is_limit_text, parse_reset_hhmm),
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
mod tests {
    use super::*;

    /// One real transcript api-error line carrying the limit banner `text`, in the
    /// exact shape Claude Code writes (`isApiErrorMessage`+`error`+`apiErrorStatus`).
    fn limit_jsonl(text: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "isApiErrorMessage": true,
            "error": "rate_limit",
            "apiErrorStatus": 429,
            "message": { "role": "assistant", "content": [ { "type": "text", "text": text } ] }
        })
        .to_string()
    }

    #[test]
    fn classify_done_from_flag() {
        assert_eq!(classify_outcome(Some("DONE\n"), false, None), Outcome::Done);
    }

    #[test]
    fn classify_blocked_from_flag() {
        assert_eq!(
            classify_outcome(Some("BLOCKED missing key"), false, None),
            Outcome::Blocked("missing key".into())
        );
    }

    #[test]
    fn classify_limit_beats_timeout() {
        // A timed-out session whose transcript shows a real rate-limit error line
        // classifies as Limit (oracle parity, ralphy.ps1:395-397) so the run
        // resumes after reset.
        let t = limit_jsonl("You've hit your usage limit");
        assert_eq!(classify_outcome(None, true, Some(&t)), Outcome::Limit(None));
    }

    #[test]
    fn classify_timeout_when_no_limit_in_transcript() {
        assert_eq!(
            classify_outcome(None, true, Some("just a normal log")),
            Outcome::Timeout
        );
    }

    #[test]
    fn classify_limit_from_transcript() {
        let t = limit_jsonl("You've reached your usage limit; resets 3:00pm");
        assert_eq!(
            classify_outcome(None, false, Some(&t)),
            Outcome::Limit(Some("15:00".into()))
        );
    }

    #[test]
    fn classify_limit_from_subagent_session_limit_transcript() {
        // The exact line Claude Code records when the session cap is hit while the
        // interactive PTY remains alive (captured from a real 429).
        let t = limit_jsonl("You've hit your session limit · resets 8:10am (America/Bahia)");
        assert_eq!(
            classify_outcome(None, false, Some(&t)),
            Outcome::Limit(Some("08:10".into()))
        );
    }

    #[test]
    fn classify_does_not_trip_on_source_that_mentions_limits() {
        // THE REGRESSION GUARD: running ralphy on a repo about rate limiting (its
        // own included) fills the transcript with tool results and assistant text
        // that say "usage limit" / "session limit" / "resets 3:00pm" — none of
        // which is a real limit. A structural detector must ignore all of it.
        let transcript = concat!(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"fn is_limit_text(text) { /* rate limit, usage limit, session limit */ }\nassert_eq!(parse_reset_hhmm(\"resets 3:00pm\"), Some(\"15:00\"));"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I'll wire the usage limit handling so a session limit auto-resumes after reset."}]}}"#,
        );
        assert_eq!(transcript_limit(transcript), None);
        // ...and a timed-out session over that transcript is a Timeout, not a Limit.
        assert_eq!(
            classify_outcome(None, true, Some(transcript)),
            Outcome::Timeout
        );
    }

    #[test]
    fn classify_stuck_when_quiet_exit() {
        assert_eq!(
            classify_outcome(None, false, Some("just a normal log")),
            Outcome::Stuck
        );
        assert_eq!(classify_outcome(None, false, None), Outcome::Stuck);
    }

    // ── is_headless_api_degraded ────────────────────────────────────────────

    #[test]
    fn headless_degraded_matcher_matches_the_retry_banners() {
        assert!(is_headless_api_degraded("Waiting for API response…"));
        // Case-insensitive, and the second distinctive substring.
        assert!(is_headless_api_degraded(
            "API Error: Server error mid-response"
        ));
    }

    #[test]
    fn headless_degraded_matcher_ignores_healthy_output() {
        // A healthy `-p` line must not match, or a busy child would starve its own
        // idle beacon and be wrongly reaped.
        assert!(!is_headless_api_degraded("Running cargo test..."));
        assert!(!is_headless_api_degraded(
            "the api call returned 200 · continuing"
        ));
    }

    // ── classify_exec_call ──────────────────────────────────────────────────

    #[test]
    fn classify_exec_not_exited_with_limit_text_is_limit() {
        let out = "You've reached your usage limit; resets 3:00pm";
        assert_eq!(
            classify_exec_call(out, false, 5),
            Some(HeadlessReason::Limit(Some("15:00".into())))
        );
    }

    #[test]
    fn classify_exec_not_exited_without_limit_is_timeout() {
        assert_eq!(
            classify_exec_call("partial output", false, 5),
            Some(HeadlessReason::Timeout)
        );
    }

    #[test]
    fn classify_exec_blocked_sentinel() {
        let out = "some work\nRALPHY_BLOCKED_EXIT missing key\nmore text";
        assert_eq!(
            classify_exec_call(out, true, 5),
            Some(HeadlessReason::Blocked("missing key".into()))
        );
    }

    #[test]
    fn classify_exec_done_via_done_sentinel() {
        let out = "all done\nRALPHY_DONE_EXIT\n";
        assert_eq!(classify_exec_call(out, true, 3), Some(HeadlessReason::Done));
    }

    #[test]
    fn classify_exec_done_via_zero_open_steps() {
        assert_eq!(
            classify_exec_call("no sentinel", true, 0),
            Some(HeadlessReason::Done)
        );
    }

    #[test]
    fn classify_exec_limit_on_natural_exit() {
        let out = "You've reached your usage limit; resets 3:00pm";
        assert_eq!(
            classify_exec_call(out, true, 2),
            Some(HeadlessReason::Limit(Some("15:00".into())))
        );
    }

    #[test]
    fn classify_exec_limit_beats_done_sentinel() {
        // The oracle checks Test-LimitText first (ralphy.ps1:283): a usage limit
        // wins even when the same exited call emitted RALPHY_DONE_EXIT, so the
        // run resumes after reset instead of closing the issue.
        let out = "RALPHY_DONE_EXIT\nYou've reached your usage limit; resets 3:00pm";
        assert_eq!(
            classify_exec_call(out, true, 0),
            Some(HeadlessReason::Limit(Some("15:00".into())))
        );
    }

    #[test]
    fn classify_exec_continue_when_no_terminal_condition() {
        assert_eq!(
            classify_exec_call("partial progress, no sentinel", true, 3),
            None
        );
    }

    // ── headless_reason_to_outcome ──────────────────────────────────────────

    #[test]
    fn headless_reason_done_maps_to_done() {
        assert_eq!(
            headless_reason_to_outcome(HeadlessReason::Done),
            Outcome::Done
        );
    }

    #[test]
    fn headless_reason_blocked_maps_to_blocked() {
        assert_eq!(
            headless_reason_to_outcome(HeadlessReason::Blocked("reason".into())),
            Outcome::Blocked("reason".into())
        );
    }

    #[test]
    fn headless_reason_timeout_maps_to_timeout() {
        assert_eq!(
            headless_reason_to_outcome(HeadlessReason::Timeout),
            Outcome::Timeout
        );
    }

    #[test]
    fn headless_reason_stuck_maps_to_stuck() {
        assert_eq!(
            headless_reason_to_outcome(HeadlessReason::Stuck),
            Outcome::Stuck
        );
    }

    #[test]
    fn headless_reason_maxcalls_maps_to_stuck() {
        assert_eq!(
            headless_reason_to_outcome(HeadlessReason::MaxCalls),
            Outcome::Stuck
        );
    }

    // ── loop-driver: stuck counter and MaxCalls ─────────────────────────────

    /// Drive the *production* `headless_step` over a scripted sequence, mirroring
    /// only the trivial `for i in 1..=max` bound in `execute_headless`. The
    /// decision logic under test is the real `headless_step`, not a copy — so a
    /// change to the loop's branching can't pass green here while diverging.
    fn run_headless_steps(
        calls: &[(Option<HeadlessReason>, bool)], // (classify result, committed) per call
        max_exec_calls: u32,
    ) -> HeadlessReason {
        let mut streak = 0u32;
        for (classified, committed) in calls.iter().take(max_exec_calls as usize) {
            match headless_step(streak, classified.clone(), *committed) {
                LoopStep::Terminal(r) => return r,
                LoopStep::Continue(s) => streak = s,
            }
        }
        HeadlessReason::MaxCalls
    }

    #[test]
    fn headless_step_passes_through_terminal_reason() {
        assert_eq!(
            headless_step(0, Some(HeadlessReason::Done), false),
            LoopStep::Terminal(HeadlessReason::Done)
        );
    }

    #[test]
    fn headless_step_commit_resets_streak() {
        assert_eq!(headless_step(1, None, true), LoopStep::Continue(0));
    }

    #[test]
    fn headless_step_second_no_commit_is_stuck() {
        assert_eq!(headless_step(0, None, false), LoopStep::Continue(1));
        assert_eq!(
            headless_step(1, None, false),
            LoopStep::Terminal(HeadlessReason::Stuck)
        );
    }

    #[test]
    fn stuck_fires_after_two_consecutive_no_commit_calls() {
        let calls = vec![
            (None, false), // call 1: streak = 1
            (None, false), // call 2: streak = 2 → Stuck
        ];
        assert_eq!(run_headless_steps(&calls, 6), HeadlessReason::Stuck);
    }

    #[test]
    fn commit_resets_no_commit_streak() {
        let calls = vec![
            (None, false), // streak = 1
            (None, true),  // committed → streak reset to 0
            (None, false), // streak = 1
            (None, false), // streak = 2 → Stuck
        ];
        assert_eq!(run_headless_steps(&calls, 6), HeadlessReason::Stuck);
    }

    #[test]
    fn loop_exhaustion_yields_maxcalls() {
        let calls: Vec<(Option<HeadlessReason>, bool)> = (0..6).map(|_| (None, true)).collect();
        assert_eq!(run_headless_steps(&calls, 6), HeadlessReason::MaxCalls);
    }

    #[test]
    fn maxcalls_outcome_is_stuck() {
        // End-to-end: loop exhaustion maps to Outcome::Stuck via headless_reason_to_outcome.
        let calls: Vec<(Option<HeadlessReason>, bool)> = (0..6).map(|_| (None, true)).collect();
        let reason = run_headless_steps(&calls, 6);
        assert_eq!(headless_reason_to_outcome(reason), Outcome::Stuck);
    }
}
