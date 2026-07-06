//! Mapping a `codex exec` call's raw end state (exit status, timeout, HEAD-diff,
//! captured output) onto the core [`Outcome`] contract (ADR-0004 D2), and the
//! single-call spawn/drain/poll/kill plumbing that produces those raw signals.

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_adapter_support::{run_headless_logged, HeadlessRun};
use ralphy_core::Outcome;

use crate::auth::{is_codex_limit_text, parse_codex_reset_hint};
use crate::CodexAgent;

/// Map an execution call's end state onto a core [`Outcome`] (ADR-0004 D2):
/// the wall timeout wins (`Timeout`); a `RALPHY_BLOCKED_EXIT <reason>` sentinel is
/// `Blocked(reason)`; a clean exit that both committed and emitted
/// `RALPHY_DONE_EXIT` is `Done`; anything else — a non-zero exit, no new commit, or
/// no sentinel — is `Stuck`. The HEAD-diff `committed` check is the same progress
/// guard the Claude headless loop uses, so a `Done` claim with no commit is
/// distrusted and downgraded to `Stuck`.
pub(crate) fn classify_codex_outcome(
    exited_cleanly: bool,
    timed_out: bool,
    committed: bool,
    out: &str,
    log: &str,
) -> Outcome {
    if timed_out {
        return Outcome::Timeout;
    }
    if let Some(reason) = ralphy_adapter_support::blocked_reason(out) {
        return Outcome::Blocked(reason);
    }
    if exited_cleanly && committed && ralphy_adapter_support::done_sentinel(out) {
        return Outcome::Done;
    }
    // A genuine usage limit makes Codex fail (non-zero or killed exit), so only a
    // non-clean exit can be a limit. Gating on the exit avoids the false positive
    // where the executed task merely echoed "usage limit" text from the source it
    // read — mirrors the Claude adapter's structural (not whole-log-substring)
    // limit detection.
    if !exited_cleanly {
        if let Some(reset) =
            ralphy_adapter_support::detect_limit(log, is_codex_limit_text, parse_codex_reset_hint)
        {
            return Outcome::Limit(reset);
        }
    }
    Outcome::Stuck
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
        run_headless_logged(cmd, prompt, timeout, &self.run_dir.join("codex.log"))
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
    fn classify_stuck_on_no_commit() {
        // A DONE claim with no new commit is distrusted (HEAD-diff progress guard).
        let out = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_codex_outcome(true, false, false, out, ""),
            Outcome::Stuck
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
}
