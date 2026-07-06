//! Mapping an `opencode run` call's raw end state onto the core [`Outcome`]
//! contract (ADR-0005 D2), and the single-call spawn/drain/poll/kill plumbing
//! that produces those raw signals.

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_adapter_support::{run_headless_logged, CompletionSignals, HeadlessRun};
use ralphy_core::Outcome;

use crate::OpenCodeAgent;

/// Extract OpenCode's [`CompletionSignals`] from a call's raw end state and delegate
/// the precedence ordering to the shared [`classify`](ralphy_adapter_support::classify)
/// ladder (ADR-0023 D1/D2). This function owns only the vendor-specific extraction:
/// the `RALPHY_DONE_EXIT`/`RALPHY_BLOCKED_EXIT` sentinel parse and the JSON `error`
/// event. OpenCode's `limit` is a structural JSON event (`parse_opencode_limit`, D5)
/// trustworthy regardless of exit — unlike Codex, no exit-code gating is needed here.
/// Ordering — including that a trustworthy limit outranks both done and timeout, and
/// that `Done` needs no commit — lives in the shared ladder, not here.
pub(crate) fn classify_opencode_outcome(
    exited_cleanly: bool,
    timed_out: bool,
    committed: bool,
    text: &str,
    saw_error: bool,
    limit: Option<Option<String>>,
) -> Outcome {
    ralphy_adapter_support::classify(CompletionSignals {
        done: ralphy_adapter_support::done_sentinel(text),
        blocked: ralphy_adapter_support::blocked_reason(text),
        limit,
        committed,
        timed_out,
        exited_ok: exited_cleanly,
        errored: saw_error,
    })
}

impl OpenCodeAgent {
    /// Spawn a single `opencode run` call, piping `prompt` on stdin and draining
    /// stdout/stderr via reader threads to avoid pipe-buffer deadlock (mirrors
    /// `CodexAgent::run_codex`). Polls `try_wait` until `timeout`; kills the child
    /// on expiry. Returns the [`HeadlessRun`] — its `stdout` is the JSON event
    /// stream the caller parses; its `log` is the combined stdout+stderr written to
    /// `run_dir/opencode.log` and used by the auth detector (auth failures often
    /// print only to stderr).
    pub(crate) fn run_opencode(
        &self,
        cmd: Command,
        prompt: &str,
        timeout: Duration,
    ) -> Result<HeadlessRun> {
        // Delegate the OS-level spawn/drain/poll/kill/collect plumbing to the
        // shared headless runner; `exited_cleanly` is a *successful* exit (the
        // status is `None` exactly when the child was killed on the wall timeout).
        // The combined log keeps stderr too — the JSON stream lives on stdout, but
        // a crash or auth failure often only prints to stderr.
        run_headless_logged(cmd, prompt, timeout, &self.run_dir.join("opencode.log"))
            .context("failed to spawn the `opencode` CLI (is it installed and on PATH?)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── classify_opencode_outcome ───────────────────────────────────────────

    #[test]
    fn classify_done_on_clean_exit_commit_and_sentinel() {
        let text = "all steps green\nRALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(true, false, true, text, false, None),
            Outcome::Done
        );
    }

    #[test]
    fn classify_done_on_no_commit() {
        // ADR-0023 D3: a commit is a progress signal, not a Done gate. A clean exit
        // with the DONE sentinel is Done even with no new commit.
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(true, false, false, text, false, None),
            Outcome::Done
        );
    }

    #[test]
    fn classify_done_run_with_limit_event_resumes() {
        // ADR-0023 D2: a trustworthy limit outranks a done claim, even on a clean,
        // committed exit — the run resumes instead of closing.
        let text = "all steps green\nRALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(
                true,
                false,
                true,
                text,
                false,
                Some(Some("2026-06-10T18:00:00Z".into()))
            ),
            Outcome::Limit(Some("2026-06-10T18:00:00Z".into()))
        );
    }

    #[test]
    fn classify_blocked_on_blocked_sentinel() {
        let text = "did some work\nRALPHY_BLOCKED_EXIT missing upstream crate\n";
        assert_eq!(
            classify_opencode_outcome(true, false, true, text, false, None),
            Outcome::Blocked("missing upstream crate".into())
        );
    }

    #[test]
    fn classify_stuck_on_non_zero_exit() {
        // A non-zero exit is Stuck even when the output carries a DONE sentinel.
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(false, false, true, text, false, None),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_stuck_on_error_event() {
        // A JSON `error` event downgrades an otherwise-clean DONE claim to Stuck.
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(true, false, true, text, true, None),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_stuck_on_no_sentinel() {
        assert_eq!(
            classify_opencode_outcome(true, false, true, "quiet exit, no sentinel", false, None),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_timeout_wins() {
        // The wall timeout wins over everything, including a DONE sentinel.
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(false, true, false, text, false, None),
            Outcome::Timeout
        );
    }

    #[test]
    fn classify_timeout_upgrades_to_limit_when_seen() {
        // A timed-out run with a limit event is upgraded to Limit(reset) (D9).
        let text = "some output\n";
        assert_eq!(
            classify_opencode_outcome(
                false,
                true,
                false,
                text,
                false,
                Some(Some("2026-06-10T18:00:00Z".into()))
            ),
            Outcome::Limit(Some("2026-06-10T18:00:00Z".into()))
        );
    }

    #[test]
    fn classify_timeout_stays_timeout_without_limit() {
        // No limit event means a hung run stays Timeout.
        let text = "some output\n";
        assert_eq!(
            classify_opencode_outcome(false, true, false, text, false, None),
            Outcome::Timeout
        );
    }

    #[test]
    fn classify_stuck_upgrades_to_limit_when_seen() {
        // A Stuck outcome is upgraded to Limit when a limit event was seen.
        let text = "no sentinel\n";
        assert_eq!(
            classify_opencode_outcome(true, false, true, text, false, Some(None)),
            Outcome::Limit(None)
        );
    }
}
