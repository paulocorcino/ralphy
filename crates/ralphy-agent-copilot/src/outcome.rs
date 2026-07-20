//! Parsing Copilot's `--output-format json` event stream for the final assistant
//! text, and mapping a call's raw end state onto the core [`Outcome`] contract via
//! the shared [`classify`](ralphy_adapter_support::classify) ladder (ADR-0023).

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_adapter_support::{CompletionSignals, HeadlessCall, HeadlessRun};
use ralphy_core::Outcome;
use serde_json::Value;

use crate::CopilotAgent;

/// Extract Copilot's final assistant message from its JSONL event stream: one
/// `{"type","data","id","timestamp","parentId","ephemeral"?}` object per line.
///
/// Three discriminators, each load-bearing (spike §2):
/// - `ephemeral: true` marks the delta/streaming records; the non-ephemeral ones
///   are the durable spine, so dropping them loses nothing — and keeping them
///   would let a half-emitted `assistant.message_delta` win the race.
/// - only `type == "assistant.message"` records carry the answer.
/// - `data.toolRequests` must be an **empty array**: a turn with pending tool
///   requests is intermediate work, not the answer. Guard on emptiness rather
///   than absence — the final turn does carry the key, as `[]`, and skipping it
///   would lose its `RALPHY_DONE_EXIT` sentinel (a Done misread as Stuck).
///
/// The LAST qualifying record wins. Robust to a truncated/empty stream: lines that
/// don't parse as JSON are skipped, so a partial last line never panics.
pub(crate) fn copilot_final_text(stdout: &str) -> String {
    let mut final_text: Option<String> = None;
    for line in stdout.lines() {
        let Ok(obj) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if obj.get("ephemeral").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if obj.get("type").and_then(Value::as_str) != Some("assistant.message") {
            continue;
        }
        let data = match obj.get("data") {
            Some(d) => d,
            None => continue,
        };
        let toolless = data
            .get("toolRequests")
            .and_then(Value::as_array)
            .map(|a| a.is_empty())
            .unwrap_or(false);
        if !toolless {
            continue;
        }
        let Some(text) = data.get("content").and_then(Value::as_str) else {
            continue;
        };
        final_text = Some(text.to_string());
    }
    final_text.unwrap_or_default()
}

/// Extract Copilot's [`CompletionSignals`] from a call's raw end state and delegate
/// the precedence ordering to the shared [`classify`](ralphy_adapter_support::classify)
/// ladder (ADR-0023 D1/D2). `final_text` is the ALREADY-extracted final assistant
/// message ([`copilot_final_text`]); this keeps the truth-table testable with plain
/// strings.
///
/// `committed` comes from the caller's HEAD-diff, never from the terminal
/// envelope's change counters: those count the vendor's own write-tool activity,
/// not repository change — in spike probe P2 the agent committed through the shell
/// tool and the envelope still reported zero lines added. That is the single most
/// dangerous false friend in this stream, and nothing here reads it.
///
/// `log` is the raw stdout+stderr the call captured; a usage limit lands there as a
/// bare error line, never in the parsed `final_text`. As with Codex and Kimi, that
/// text scan is only trusted on a non-clean exit — a genuine limit fails the
/// process, so a clean exit proves the phrase was merely echoed. `exit_code` is
/// unused for limit detection: Copilot has no semantic exit code equivalent to
/// Kimi's `75 = RETRYABLE` (spike §3), so every limit arrives as exit 1 + text.
pub(crate) fn classify_copilot_outcome(
    exited_cleanly: bool,
    timed_out: bool,
    committed: bool,
    _exit_code: Option<i32>,
    final_text: &str,
    log: &str,
) -> Outcome {
    let limit = if !exited_cleanly {
        // No reset hint: Copilot's limit surface is unobserved (D11), so the
        // ADR-0030 synthetic cadence handles the wait.
        ralphy_adapter_support::detect_limit(log, crate::auth::is_copilot_limit_text, |_| None)
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

/// D11's preflight: `continueOnAutoMode` is a vendor-internal retry that
/// silently switches model and hides a rate limit from Ralphy, so it is asserted
/// BEFORE a child is spawned — no token is spent on a run that cannot be trusted.
/// `None` (no config file, or an unreadable one) is a pass; see
/// [`crate::guards::continue_on_auto_mode_violation`].
pub(crate) fn preflight(config_src: Option<&str>) -> Result<()> {
    if let Some(msg) = config_src.and_then(crate::guards::continue_on_auto_mode_violation) {
        anyhow::bail!("{msg}");
    }
    Ok(())
}

impl CopilotAgent {
    /// Spawn a single headless `copilot` call, piping `prompt` on stdin and
    /// draining stdout/stderr via the shared headless runner (avoids pipe-buffer
    /// deadlock). The combined log is written to `run_dir/copilot.log`; the caller
    /// reads the final assistant text from the returned [`HeadlessRun::stdout`].
    /// The single [`HeadlessCall`] site in the crate (ADR-0040 Tier 1).
    pub(crate) fn run_copilot(
        &self,
        cmd: Command,
        prompt: &str,
        timeout: Duration,
    ) -> Result<HeadlessRun> {
        // FIRST, before `HeadlessCall::new`: a D11 violation must cost no child
        // and no tokens. A read error is a pass — the file is machine-managed.
        let config =
            crate::guards::copilot_config_path().and_then(|p| std::fs::read_to_string(p).ok());
        preflight(config.as_deref())?;
        HeadlessCall::new(cmd, prompt, timeout, &self.run_dir.join("copilot.log"))
            .idle_minutes(self.budget.idle_minutes)
            .run()
            .context("failed to spawn the `copilot` CLI (is it installed and on PATH?)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D11 bails BEFORE any child is spawned — the assertion is on the extracted
    /// helper precisely so it needs no real process.
    #[test]
    fn run_copilot_preflight_bails_before_spawn() {
        let err = preflight(Some(r#"{"continueOnAutoMode": true}"#))
            .expect_err("continueOnAutoMode must abort before the spawn");
        assert!(err.to_string().contains("continueOnAutoMode"), "{err}");

        // No config, an empty config, and an unparsable one all pass.
        assert!(preflight(None).is_ok());
        assert!(preflight(Some("{}")).is_ok());
        assert!(preflight(Some("not json")).is_ok());
    }

    const ANSWER: &str = r#"{"type":"assistant.message","id":"a1","data":{"model":"claude-sonnet-5","content":"all green\nRALPHY_DONE_EXIT","toolRequests":[],"outputTokens":75}}"#;

    /// The terminal envelope from spike §2, with the change counters ZEROED even
    /// though the agent committed through the shell tool. Lives in the test module
    /// so `no_code_changes_read` can scan the production half for the same literal.
    const RESULT_ZEROED: &str = r#"{"type":"result","sessionId":"d911b7f0","exitCode":0,"usage":{"premiumRequests":0.33,"codeChanges":{"linesAdded":0,"linesRemoved":0,"filesModified":[]}}}"#;

    #[test]
    fn copilot_final_text_returns_last_toolless_assistant() {
        let working = r#"{"type":"assistant.message","id":"a0","data":{"content":"working","toolRequests":[{"name":"shell"}]}}"#;
        let tool = r#"{"type":"tool.execution_complete","id":"t0","data":{"ok":true}}"#;
        let text = copilot_final_text(&format!("{working}\n{tool}\n{ANSWER}\n{RESULT_ZEROED}\n"));
        assert!(text.ends_with("RALPHY_DONE_EXIT"), "got: {text:?}");
        assert!(
            !text.contains("working"),
            "text from a toolRequests turn must be excluded: {text:?}"
        );
    }

    #[test]
    fn copilot_final_text_ignores_a_trailing_tool_request_turn() {
        // A toolRequests turn AFTER the answer must NOT win: this discriminates the
        // "skip pending-tool turns" rule from "just take the last assistant line".
        let trailing = r#"{"type":"assistant.message","id":"a2","data":{"content":"more","toolRequests":[{"name":"shell"}]}}"#;
        let text = copilot_final_text(&format!("{ANSWER}\n{trailing}\n"));
        assert!(text.ends_with("RALPHY_DONE_EXIT"), "got: {text:?}");
        assert!(
            !text.contains("more"),
            "trailing toolRequests turn must lose: {text:?}"
        );
    }

    #[test]
    fn copilot_final_text_drops_ephemeral_records() {
        // A streaming delta emitted AFTER the durable answer must not win.
        let partial = r#"{"type":"assistant.message","id":"a3","ephemeral":true,"data":{"content":"partial","toolRequests":[]}}"#;
        let text = copilot_final_text(&format!("{ANSWER}\n{partial}\n"));
        assert!(text.ends_with("RALPHY_DONE_EXIT"), "got: {text:?}");
        assert!(
            !text.contains("partial"),
            "ephemeral records must be dropped: {text:?}"
        );
    }

    #[test]
    fn copilot_final_text_survives_malformed_and_empty() {
        assert_eq!(copilot_final_text(""), "");
        assert_eq!(copilot_final_text("\n\n{not json"), "");
        // A valid answer followed by a truncated tail: the answer still wins.
        assert!(copilot_final_text(&format!("{ANSWER}\n{{trunc")).ends_with("RALPHY_DONE_EXIT"));
        // An assistant.message with no toolRequests key at all is not the answer.
        let keyless = r#"{"type":"assistant.message","id":"a4","data":{"content":"nope"}}"#;
        assert_eq!(copilot_final_text(keyless), "");
    }

    // ── the ADR-0023 ladder ─────────────────────────────────────────────────

    #[test]
    fn classify_blocked_on_blocked_sentinel() {
        assert_eq!(
            classify_copilot_outcome(
                true,
                false,
                true,
                Some(0),
                "work\nRALPHY_BLOCKED_EXIT missing crate",
                ""
            ),
            Outcome::Blocked("missing crate".into())
        );
    }

    #[test]
    fn classify_done_on_clean_exit_commit_and_sentinel() {
        assert_eq!(
            classify_copilot_outcome(
                true,
                false,
                true,
                Some(0),
                "all green\nRALPHY_DONE_EXIT",
                ""
            ),
            Outcome::Done
        );
    }

    #[test]
    fn classify_timeout_wins() {
        assert_eq!(
            classify_copilot_outcome(false, true, false, None, "RALPHY_DONE_EXIT", ""),
            Outcome::Timeout
        );
    }

    #[test]
    fn classify_stuck_on_non_zero_exit() {
        assert_eq!(
            classify_copilot_outcome(false, false, true, Some(1), "RALPHY_DONE_EXIT", ""),
            Outcome::Stuck
        );
    }

    #[test]
    fn classify_limit_maps_to_limit_none() {
        // No reset hint is recoverable, so `Limit(None)` — which is what keys the
        // ADR-0030 synthetic cadence.
        let log = "Error: API rate limit exceeded for this token.";
        assert_eq!(
            classify_copilot_outcome(false, false, false, Some(1), "", log),
            Outcome::Limit(None)
        );
    }

    #[test]
    fn classify_limit_ignored_on_clean_exit() {
        // A clean exit proves the phrase was merely echoed by the agent's prose.
        let log = "the issue asks us to handle a rate limit gracefully";
        assert_eq!(
            classify_copilot_outcome(true, false, true, Some(0), "RALPHY_DONE_EXIT", log),
            Outcome::Done
        );
    }

    #[test]
    fn classify_done_ignores_zeroed_code_changes() {
        // The false friend: the envelope reports zero changes because the work went
        // through the shell tool, but HEAD advanced. `committed` comes from the
        // HEAD-diff, so the outcome is Done.
        let stdout = format!("{ANSWER}\n{RESULT_ZEROED}\n");
        let final_text = copilot_final_text(&stdout);
        assert_eq!(
            classify_copilot_outcome(true, false, true, Some(0), &final_text, &stdout),
            Outcome::Done
        );
    }

    /// The production half of this file must never so much as name the envelope's
    /// change counters — reading them would resurrect the false friend above.
    #[test]
    fn no_code_changes_read() {
        let production = include_str!("outcome.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(
            !production.contains("codeChanges"),
            "the change counters must never be consulted (spike §2)"
        );
    }
}
