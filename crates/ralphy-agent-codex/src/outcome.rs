//! Mapping a `codex exec` call's raw end state (exit status, timeout, HEAD-diff,
//! captured output) onto the core [`Outcome`] contract (ADR-0004 D2), and the
//! single-call spawn/drain/poll/kill plumbing that produces those raw signals.

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_adapter_support::{CompletionSignals, HeadlessCall, HeadlessRun};
use ralphy_core::Outcome;

use crate::auth::{is_codex_auth_error, is_codex_limit_text, parse_codex_reset_hint};
use crate::CodexAgent;

/// The headless **degraded-line** matcher, handed to the shared runner's
/// `degraded_line` seam. Conservative, keyed on codex's transient-error
/// vocabulary `(indicative — refine against a captured degraded run)`: a stream
/// hiccup the CLI retries internally. Terminal auth/limit lines are excluded via
/// the guard below. Deliberately does NOT key on `reconnecting`: the runner feeds
/// the predicate ONE line at a time, and a logged-out codex run prints its
/// `Reconnecting... 5/5` retries on lines SEPARATE from the `401 Unauthorized` —
/// so a `reconnecting` token would read those auth-retry lines as degraded even
/// though the auth guard (which only sees the 401 line) can't reach them. A false
/// positive that reaped healthy work is the only unsafe failure mode, so narrowness
/// is the design goal; a miss is safe.
fn is_codex_api_degraded(line: &str) -> bool {
    if is_codex_auth_error(line) || is_codex_limit_text(line) {
        return false;
    }
    let l = line.to_ascii_lowercase();
    l.contains("stream error") || l.contains("server error")
}

/// Extract Codex's [`CompletionSignals`] from a call's raw end state and delegate
/// the precedence ordering to the shared [`classify`](ralphy_adapter_support::classify)
/// ladder (ADR-0023 D1/D2). This function owns only the vendor-specific extraction:
/// the `RALPHY_DONE_EXIT`/`RALPHY_BLOCKED_EXIT` sentinel parse, `exited_ok` = a
/// clean exit, and — crucially — that a usage `limit` is only trustworthy on a
/// non-clean exit (a genuine limit fails the process, so a clean exit *is* the
/// "not a limit" proof; this avoids the false positive where the task merely echoed
/// "usage limit" text from a source it read). Ordering — including that a
/// trustworthy limit outranks both done and timeout, and that `Done` needs no
/// commit — lives in the shared ladder, not here.
pub(crate) fn classify_codex_outcome(
    exited_cleanly: bool,
    timed_out: bool,
    committed: bool,
    out: &str,
    log: &str,
) -> Outcome {
    let limit = if !exited_cleanly {
        ralphy_adapter_support::detect_limit(log, is_codex_limit_text, parse_codex_reset_hint)
    } else {
        None
    };
    ralphy_adapter_support::classify(CompletionSignals {
        done: ralphy_adapter_support::done_sentinel(out),
        blocked: ralphy_adapter_support::blocked_reason(out),
        limit,
        committed,
        timed_out,
        exited_ok: exited_cleanly,
        errored: false,
    })
}

impl CodexAgent {
    /// Spawn a single `codex exec` call, piping `prompt` on stdin and draining
    /// stdout/stderr via reader threads to avoid pipe-buffer deadlock (mirrors
    /// `ClaudeAgent::run_headless_call`). Polls `try_wait` until `timeout`; kills
    /// the child on expiry. Returns the [`HeadlessRun`] (its `exited_cleanly` /
    /// `timed_out` / `log`) — the combined log is also written to `run_dir/codex.log`;
    /// the agent's final message is read from the `-o` file by the caller.
    pub(crate) fn run_codex(
        &self,
        cmd: Command,
        prompt: &str,
        timeout: Duration,
    ) -> Result<HeadlessRun> {
        // Delegate the OS-level spawn/drain/poll/kill/collect plumbing to the
        // shared headless runner; Codex's `exited_cleanly` (a *successful* exit,
        // not merely "not timed out") is recovered from the returned exit status,
        // which is `None` exactly when the child was killed on the wall timeout.
        // The idle watchdog reaps a child that has gone silent past its window,
        // which the wall `timeout` cannot do now that the per-issue cap is opt-in
        // and unbounded by default (docs/adr/0038).
        HeadlessCall::new(cmd, prompt, timeout, &self.run_dir.join("codex.log"))
            .idle_minutes(self.budget.idle_minutes)
            .degraded_line(is_codex_api_degraded)
            .run()
            .context("failed to spawn the `codex` CLI (is it installed and on PATH?)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_done_on_clean_exit_commit_and_sentinel() {
        let out = "all steps green\nRALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_codex_outcome(true, false, true, out, ""),
            Outcome::Done
        );
    }

    #[test]
    fn classify_blocked_on_blocked_sentinel() {
        let out = "did some work\nRALPHY_BLOCKED_EXIT missing upstream crate\n";
        assert_eq!(
            classify_codex_outcome(true, false, true, out, ""),
            Outcome::Blocked("missing upstream crate".into())
        );
    }

    #[test]
    fn classify_stuck_on_non_zero_exit() {
        // A non-zero exit is Stuck even when the output carries a DONE sentinel.
        let out = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_codex_outcome(false, false, true, out, ""),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_done_on_no_commit() {
        // ADR-0023 D3: a commit is a progress signal, not a Done gate. A clean exit
        // with the DONE sentinel is Done even with no new commit.
        let out = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_codex_outcome(true, false, false, out, ""),
            Outcome::Done
        );
    }

    #[test]
    fn classify_stuck_on_no_sentinel() {
        assert_eq!(
            classify_codex_outcome(true, false, true, "quiet exit, no sentinel", ""),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_timeout_wins() {
        // The wall timeout wins over everything, including a DONE sentinel.
        let out = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_codex_outcome(false, true, false, out, ""),
            Outcome::Timeout
        );
    }

    // ── is_codex_api_degraded ───────────────────────────────────────────────

    #[test]
    fn codex_degraded_matches_transient_error() {
        assert!(is_codex_api_degraded(
            "ERROR codex_api: stream error, retrying"
        ));
        assert!(is_codex_api_degraded("Server error, backing off"));
    }

    #[test]
    fn codex_degraded_ignores_healthy_and_terminal_lines() {
        assert!(!is_codex_api_degraded(
            "all steps green\nRALPHY_DONE_EXIT\n"
        ));
        // Per-line, as the runner delivers them: a logged-out run prints the 401 and
        // its `Reconnecting...` retries on SEPARATE lines. The 401 line is caught by
        // the auth guard; the reconnect line must ALSO not read as degraded (the
        // matcher deliberately does not key on `reconnecting`).
        assert!(!is_codex_api_degraded(
            "ERROR: unexpected status 401 Unauthorized: Missing bearer or basic \
             authentication in header"
        ));
        assert!(!is_codex_api_degraded("ERROR: Reconnecting... 5/5"));
        // A usage limit is terminal, not degraded.
        assert!(!is_codex_api_degraded(
            "You've hit your usage limit. Try again at 2026-06-09T18:00:00Z."
        ));
    }

    // ── classify_codex_outcome — limit branch ───────────────────────────────

    #[test]
    fn classify_limit_with_reset_hint() {
        let log = "You've hit your usage limit. Try again at 2026-06-09T18:00:00Z.";
        assert_eq!(
            classify_codex_outcome(false, false, false, "", log),
            Outcome::Limit(Some("2026-06-09T18:00:00Z".into()))
        );
    }

    #[test]
    fn classify_limit_bare_when_no_hint() {
        let log = "Error: usage limit exceeded.";
        assert_eq!(
            classify_codex_outcome(false, false, false, "", log),
            Outcome::Limit(None)
        );
    }

    #[test]
    fn classify_limit_upgrades_timeout() {
        // ADR-0023 D4 P3: a trustworthy limit on a timed-out run yields Limit, not
        // Timeout — resume-after-reset is the conservative error.
        assert_eq!(
            classify_codex_outcome(
                false,
                true,
                false,
                "",
                "You've hit your usage limit. Try again at 2026-06-09T18:00:00Z."
            ),
            Outcome::Limit(Some("2026-06-09T18:00:00Z".into()))
        );
    }
}
