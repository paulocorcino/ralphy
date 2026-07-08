//! Parsing Kimi's `stream-json` role-JSONL for the final assistant text, and
//! mapping a `kimi --print` call's raw end state onto the core [`Outcome`]
//! contract via the shared [`classify`](ralphy_adapter_support::classify) ladder
//! (ADR-0023).

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_adapter_support::{run_headless_logged, CompletionSignals, HeadlessRun};
use ralphy_core::Outcome;
use serde_json::Value;

use crate::KimiAgent;

/// Extract Kimi's final assistant message from its `stream-json` output: coarse
/// role-JSONL, one object per line. The final message is the LAST `role:assistant`
/// object that carries NO pending `tool_calls` (a tool-call turn is intermediate,
/// not the answer); its `content[]` `text` parts are concatenated.
///
/// Robust to a truncated/empty stream: lines that don't parse as JSON are skipped,
/// so a partial last line never panics (`from_str` returns `Err`, we move on).
pub(crate) fn kimi_final_text(stdout: &str) -> String {
    let mut final_text: Option<String> = None;
    for line in stdout.lines() {
        let Ok(obj) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if obj.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        // A turn with PENDING tool_calls is intermediate work, not the final answer.
        // Guard on a NON-EMPTY array, not merely non-null: a terminal turn that
        // carries an empty `tool_calls: []` is still the final answer, and skipping
        // it would lose its `RALPHY_DONE_EXIT` sentinel (Done misread as Stuck).
        let has_pending_tool_calls = obj
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        if has_pending_tool_calls {
            continue;
        }
        let Some(parts) = obj.get("content").and_then(Value::as_array) else {
            continue;
        };
        let text: String = parts
            .iter()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect();
        // Keep the LAST qualifying assistant turn (overwrite as we go).
        final_text = Some(text);
    }
    final_text.unwrap_or_default()
}

/// Extract Kimi's [`CompletionSignals`] from a call's raw end state and delegate
/// the precedence ordering to the shared [`classify`](ralphy_adapter_support::classify)
/// ladder (ADR-0023 D1/D2). `final_text` is the ALREADY-extracted final assistant
/// message ([`kimi_final_text`]); this keeps the truth-table testable with plain
/// strings. `exit_code == Some(75)` maps to `Limit(None)` — no structured reset
/// hint at the chat level (ADR-0028 D9); the auth/permanent exit-1 case is handled
/// by the scaffold's `is_kimi_auth_error` bail, not here.
pub(crate) fn classify_kimi_outcome(
    exited_cleanly: bool,
    timed_out: bool,
    committed: bool,
    exit_code: Option<i32>,
    final_text: &str,
) -> Outcome {
    let limit = if exit_code == Some(75) {
        Some(None)
    } else {
        None
    };
    ralphy_adapter_support::classify(CompletionSignals {
        done: ralphy_adapter_support::done_sentinel(final_text),
        blocked: ralphy_adapter_support::blocked_reason(final_text),
        limit,
        committed,
        timed_out,
        exited_ok: exited_cleanly,
        errored: false,
    })
}

impl KimiAgent {
    /// Spawn a single `kimi --print` call, piping `prompt` on stdin and draining
    /// stdout/stderr via the shared headless runner (avoids pipe-buffer deadlock).
    /// The combined log is written to `run_dir/kimi.log`; the caller reads the
    /// final assistant text from the returned [`HeadlessRun::stdout`].
    pub(crate) fn run_kimi(
        &self,
        cmd: Command,
        prompt: &str,
        timeout: Duration,
    ) -> Result<HeadlessRun> {
        run_headless_logged(cmd, prompt, timeout, &self.run_dir.join("kimi.log"))
            .context("failed to spawn the `kimi` CLI (is it installed and on PATH?)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kimi_final_text_returns_last_toolless_assistant() {
        let line1 = r#"{"role":"assistant","content":[{"type":"text","text":"working"}],"tool_calls":[{"type":"function","id":"tool_1","function":{"name":"WriteFile","arguments":"{}"}}]}"#;
        let line2 = r#"{"role":"tool","content":"ok","tool_call_id":"tool_1"}"#;
        let line3 = r#"{"role":"assistant","content":[{"type":"think","think":"done"},{"type":"text","text":"all green\nRALPHY_DONE_EXIT"}]}"#;
        let stdout = format!("{line1}\n{line2}\n{line3}\n");
        let text = kimi_final_text(&stdout);
        assert!(
            text.ends_with("RALPHY_DONE_EXIT"),
            "final text must be the last tool-less assistant turn: {text:?}"
        );
        // The tool-call turn's "working" text must NOT leak in — proves the
        // no-tool_calls rule, not a naive substring grep.
        assert!(
            !text.contains("working"),
            "text from a tool_calls turn must be excluded: {text:?}"
        );
    }

    #[test]
    fn kimi_final_text_ignores_a_trailing_tool_call_turn() {
        // A tool_calls turn AFTER the answer must NOT win: this discriminates the
        // "skip tool_calls turns" rule from "just take the last assistant line".
        let answer = r#"{"role":"assistant","content":[{"type":"text","text":"all green\nRALPHY_DONE_EXIT"}]}"#;
        let trailing = r#"{"role":"assistant","content":[{"type":"text","text":"more"}],"tool_calls":[{"type":"function","id":"t2","function":{"name":"Bash","arguments":"{}"}}]}"#;
        let text = kimi_final_text(&format!("{answer}\n{trailing}\n"));
        assert!(text.ends_with("RALPHY_DONE_EXIT"), "got: {text:?}");
        assert!(
            !text.contains("more"),
            "trailing tool_calls turn must lose: {text:?}"
        );
    }

    #[test]
    fn kimi_final_text_keeps_empty_tool_calls_answer() {
        // A terminal turn with an empty `tool_calls: []` is still the final answer.
        let line = r#"{"role":"assistant","content":[{"type":"text","text":"done\nRALPHY_DONE_EXIT"}],"tool_calls":[]}"#;
        assert!(kimi_final_text(line).ends_with("RALPHY_DONE_EXIT"));
    }

    #[test]
    fn kimi_final_text_survives_malformed_and_empty() {
        // A truncated last line and blank lines must not panic; empty in → empty out.
        assert_eq!(kimi_final_text(""), "");
        assert_eq!(kimi_final_text("\n\n{not json"), "");
        // A valid answer line followed by a truncated tail: the answer still wins.
        let answer =
            r#"{"role":"assistant","content":[{"type":"text","text":"ok\nRALPHY_DONE_EXIT"}]}"#;
        assert!(kimi_final_text(&format!("{answer}\n{{trunc")).ends_with("RALPHY_DONE_EXIT"));
    }

    #[test]
    fn classify_done_on_clean_exit_commit_and_sentinel() {
        assert_eq!(
            classify_kimi_outcome(true, false, true, Some(0), "all green\nRALPHY_DONE_EXIT"),
            Outcome::Done
        );
    }

    #[test]
    fn classify_blocked_on_blocked_sentinel() {
        assert_eq!(
            classify_kimi_outcome(
                true,
                false,
                true,
                Some(0),
                "work\nRALPHY_BLOCKED_EXIT missing crate"
            ),
            Outcome::Blocked("missing crate".into())
        );
    }

    #[test]
    fn classify_timeout_wins() {
        assert_eq!(
            classify_kimi_outcome(false, true, false, None, "RALPHY_DONE_EXIT"),
            Outcome::Timeout
        );
    }

    #[test]
    fn classify_stuck_on_non_zero_exit() {
        assert_eq!(
            classify_kimi_outcome(false, false, true, Some(1), "RALPHY_DONE_EXIT"),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_done_on_no_commit() {
        // A commit is a progress signal, not a Done gate (ADR-0023 D3).
        assert_eq!(
            classify_kimi_outcome(true, false, false, Some(0), "RALPHY_DONE_EXIT"),
            Outcome::Done
        );
    }

    #[test]
    fn classify_limit_on_exit_75() {
        // The literal DONE sentinel is present, proving the limit outranks a would-be Done.
        assert_eq!(
            classify_kimi_outcome(false, false, false, Some(75), "RALPHY_DONE_EXIT"),
            Outcome::Limit(None)
        );
    }

    #[test]
    fn classify_limit_beats_timeout() {
        assert_eq!(
            classify_kimi_outcome(false, true, false, Some(75), ""),
            Outcome::Limit(None)
        );
    }
}
