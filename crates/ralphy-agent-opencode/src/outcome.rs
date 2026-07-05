//! Mapping an `opencode run` call's raw end state onto the core [`Outcome`]
//! contract (ADR-0005 D2), and the single-call spawn/drain/poll/kill plumbing
//! that produces those raw signals.

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_adapter_support::run_headless_logged;
use ralphy_core::Outcome;

use crate::OpenCodeAgent;

/// Map an execution call's end state onto a core [`Outcome`] (ADR-0005 D2): the
/// wall timeout wins, but a `limit` event (D9) upgrades `Timeout` to
/// `Outcome::Limit(reset)` and the `Stuck` fallthrough to `Outcome::Limit` when
/// present; a `RALPHY_BLOCKED_EXIT <reason>` sentinel is `Blocked(reason)`; a
/// clean exit that committed, saw no `error` event, and emitted `RALPHY_DONE_EXIT`
/// is `Done`; anything else is `Stuck`. The HEAD-diff `committed` check is the
/// progress guard the Claude headless loop and the Codex adapter already use:
/// OpenCode makes internal snapshots, not git commits, so a `Done` claim with no
/// commit is distrusted and downgraded.
pub(crate) fn classify_opencode_outcome(
    exited_cleanly: bool,
    timed_out: bool,
    committed: bool,
    text: &str,
    saw_error: bool,
    limit: Option<Option<String>>,
) -> Outcome {
    if timed_out {
        return limit.map(Outcome::Limit).unwrap_or(Outcome::Timeout);
    }
    if let Some(reason) = ralphy_adapter_support::blocked_reason(text) {
        return Outcome::Blocked(reason);
    }
    if exited_cleanly && committed && !saw_error && ralphy_adapter_support::done_sentinel(text) {
        return Outcome::Done;
    }
    if let Some(reset) = limit {
        return Outcome::Limit(reset);
    }
    Outcome::Stuck
}

impl OpenCodeAgent {
    /// Spawn a single `opencode run` call, piping `prompt` on stdin and draining
    /// stdout/stderr via reader threads to avoid pipe-buffer deadlock (mirrors
    /// `CodexAgent::run_codex`). Polls `try_wait` until `timeout`; kills the child
    /// on expiry. Returns `(exited_cleanly, timed_out, stdout_text, log)` — stdout
    /// is the JSON event stream the caller parses; `log` is the combined
    /// stdout+stderr written to `run_dir/opencode.log` and used by the auth
    /// detector (auth failures often print only to stderr).
    pub(crate) fn run_opencode(
        &self,
        cmd: Command,
        prompt: &str,
        timeout: Duration,
    ) -> Result<(bool, bool, String, String)> {
        // Delegate the OS-level spawn/drain/poll/kill/collect plumbing to the
        // shared headless runner; `exited_cleanly` is a *successful* exit (the
        // status is `None` exactly when the child was killed on the wall timeout).
        // The combined log keeps stderr too — the JSON stream lives on stdout, but
        // a crash or auth failure often only prints to stderr.
        let r = run_headless_logged(cmd, prompt, timeout, &self.run_dir.join("opencode.log"))
            .context("failed to spawn the `opencode` CLI (is it installed and on PATH?)")?;
        Ok((r.exited_cleanly, r.timed_out, r.stdout, r.log))
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
    fn classify_stuck_on_no_commit() {
        // A DONE claim with no new commit is distrusted (HEAD-diff progress guard).
        let text = "RALPHY_DONE_EXIT\n";
        assert_eq!(
            classify_opencode_outcome(true, false, false, text, false, None),
            Outcome::Stuck
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
