//! Folding Cursor's `--output-format stream-json` record stream into the signals
//! the shared [`classify`](ralphy_adapter_support::classify) ladder needs
//! (ADR-0023), and the crate's single child-spawning seam.
//!
//! The fold exists because on this vendor **absence is a signal**: the docs state
//! that on error the stream *"may end early without a terminal event"*, so the
//! record count and the presence of the envelope discriminate a preflight
//! rejection from a truncation from a clean run (ADR-0042 D3).

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_adapter_support::{CompletionSignals, HeadlessCall, HeadlessRun};
use ralphy_core::Outcome;
use serde_json::Value;

use crate::CursorAgent;

/// What one call's stdout reduces to. Everything the classifier and the run report
/// need, extracted once so the truth table tests against plain strings.
#[derive(Debug, Default)]
pub(crate) struct CursorFold {
    /// `result.result` — the final assistant message, duplicated verbatim into the
    /// envelope, so the "last toolless assistant record" heuristic is unnecessary.
    pub(crate) final_text: String,
    /// `system/init.session_id`, checked against the minted id (D10).
    pub(crate) session_id: Option<String>,
    pub(crate) is_error: bool,
    /// `None` when no envelope arrived. An UNKNOWN value is not success — neither
    /// `is_error: true` nor any other `subtype` was ever reproduced (D3), so the
    /// parser handles them defensively rather than optimistically.
    pub(crate) subtype: Option<String>,
    /// Whether the terminal `result` record arrived at all. `false` is a failure
    /// signal in its own right.
    pub(crate) saw_envelope: bool,
    /// Tool calls whose result was `failure` rather than `success`. A failed tool
    /// call is **not** a failed run — the envelope still reports success — so these
    /// feed the degraded note, never the outcome.
    pub(crate) failed_tool_calls: Vec<String>,
    /// Tool calls the operator's own `permissions.deny` blocked (D7). Same
    /// treatment, different cause: the run is green and quietly did less.
    pub(crate) denied_tool_calls: Vec<String>,
}

impl CursorFold {
    /// How many records parsed at all. Zero plus a non-zero exit is a preflight
    /// rejection, not a truncation (D3 rule 2).
    pub(crate) fn saw_no_records(&self) -> bool {
        !self.saw_envelope
            && self.session_id.is_none()
            && self.failed_tool_calls.is_empty()
            && self.denied_tool_calls.is_empty()
            && self.final_text.is_empty()
    }

    /// The operator-facing note for a green run that quietly did less: a tool call
    /// that failed, or one their deny list blocked. `None` when nothing bit.
    pub(crate) fn degraded_note(&self) -> Option<String> {
        if self.failed_tool_calls.is_empty() && self.denied_tool_calls.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.failed_tool_calls.is_empty() {
            parts.push(format!(
                "failed tool calls: {}",
                self.failed_tool_calls.join("; ")
            ));
        }
        if !self.denied_tool_calls.is_empty() {
            parts.push(format!(
                "blocked by your Cursor permissions.deny: {}",
                self.denied_tool_calls.join("; ")
            ));
        }
        Some(parts.join(" | "))
    }
}

/// Pull the human-readable command out of a tool-result discriminator, falling back
/// to the record's own shape so a nameless call still leaves a trace.
fn describe(discriminator: &Value, fallback: &str) -> String {
    for key in ["command", "path", "cmd"] {
        if let Some(s) = discriminator.get(key).and_then(Value::as_str) {
            return s.to_string();
        }
    }
    fallback.to_string()
}

/// Walk a `tool_call` record for the three result discriminators the vendor uses —
/// `success`, `failure`, `permissionDenied` — wherever they are nested.
///
/// The search is structural rather than path-literal because the tool wrapper key
/// varies per tool (`shellToolCall`, `editToolCall`, `readToolCall`), and a
/// hardcoded path would silently stop discriminating the day a new tool appears —
/// which reads as a clean run, the failure direction that hides work not done.
fn collect_tool_results(v: &Value, failed: &mut Vec<String>, denied: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            if let Some(Value::Object(result)) = map.get("result") {
                if let Some(d) = result.get("permissionDenied") {
                    denied.push(describe(d, "a denied tool call"));
                }
                if let Some(d) = result.get("failure") {
                    failed.push(describe(d, "a failed tool call"));
                }
            }
            for child in map.values() {
                collect_tool_results(child, failed, denied);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_tool_results(child, failed, denied);
            }
        }
        _ => {}
    }
}

/// Fold one call's stdout. Lines that do not parse as JSON are skipped, so a
/// truncated last line — the ordinary shape of a killed child — never panics.
pub(crate) fn fold_cursor_stream(stdout: &str) -> CursorFold {
    let mut fold = CursorFold::default();
    for line in stdout.lines() {
        let Ok(obj) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let ty = obj.get("type").and_then(Value::as_str).unwrap_or_default();
        let subtype = obj
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match (ty, subtype) {
            ("system", "init") => {
                fold.session_id = obj
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            ("result", _) => {
                fold.saw_envelope = true;
                fold.subtype = obj
                    .get("subtype")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                fold.is_error = obj
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Some(text) = obj.get("result").and_then(Value::as_str) {
                    fold.final_text = text.to_string();
                }
                if let Some(sid) = obj.get("session_id").and_then(Value::as_str) {
                    fold.session_id.get_or_insert_with(|| sid.to_string());
                }
            }
            ("tool_call", "completed") => {
                collect_tool_results(
                    &obj,
                    &mut fold.failed_tool_calls,
                    &mut fold.denied_tool_calls,
                );
            }
            _ => {}
        }
    }
    fold
}

/// The exit code a `SIGINT` produces. The vendor's ONE semantic exit code, and it
/// matters because it is the shape Ralphy's own budget and idle watchdogs produce
/// when *they* stop the child (ADR-0038): "we stopped it" must not be reported as
/// "it crashed".
const INTERRUPTED: i32 = 130;

/// Extract Cursor's [`CompletionSignals`] and delegate the precedence ordering to
/// the shared ladder (ADR-0023 D1/D2).
///
/// `committed` comes from the caller's HEAD-diff. It is never derived from the
/// stream: `shellToolCall.result.success` carries no file-change data at all, so
/// work done through the shell reports zero progress (spike §2).
///
/// A run is `errored` unless the envelope arrived AND said `success`. That covers
/// three shapes with one rule — `is_error: true`, an unknown `subtype`, and an
/// envelope that never came — and it is deliberately the pessimistic direction:
/// neither of the first two was ever reproduced, so the parser must not assume the
/// shape it happens to have seen is the only one.
pub(crate) fn classify_cursor_outcome(
    fold: &CursorFold,
    exited_cleanly: bool,
    timed_out: bool,
    committed: bool,
    exit_code: Option<i32>,
) -> Outcome {
    let interrupted = exit_code == Some(INTERRUPTED);
    let succeeded =
        fold.saw_envelope && !fold.is_error && fold.subtype.as_deref() == Some("success");
    ralphy_adapter_support::classify(CompletionSignals {
        done: ralphy_adapter_support::done_sentinel(&fold.final_text),
        blocked: ralphy_adapter_support::blocked_reason(&fold.final_text),
        // D13 is open: no limit signature has ever been observed on this vendor, so
        // a limit surfaces as an ordinary failure rather than a guessed phrase match.
        limit: None,
        committed,
        // An interrupt IS Ralphy stopping the child, so it lands on `Timeout`
        // rather than falling through the ladder to `Stuck`.
        timed_out: timed_out || interrupted,
        exited_ok: exited_cleanly && !interrupted,
        errored: !succeeded,
    })
}

impl CursorAgent {
    /// Spawn a single headless `cursor-agent` call, piping `prompt` on stdin and
    /// draining stdout/stderr via the shared headless runner. The crate's single
    /// [`HeadlessCall`] site (ADR-0040 Tier 1).
    ///
    /// **Cross-path invariant:** D6's indexing gate and D17's config seeding both
    /// run BEFORE `HeadlessCall::new`, on every path including the error ones. A
    /// child spawned before the gate returns `Ok` has already uploaded the
    /// repository, so "we refused afterwards" is not a refusal.
    pub(crate) fn run_cursor(
        &self,
        cmd: Command,
        prompt: &str,
        timeout: Duration,
        work_dir: &Path,
    ) -> Result<HeadlessRun> {
        crate::guards::indexing_gate(work_dir, self.allow_indexing)?;
        crate::command::seed_cursor_config_dir(
            crate::command::operator_config_dir().as_deref(),
            &self.config_dir(),
        )?;
        HeadlessCall::new(cmd, prompt, timeout, &self.run_dir.join("cursor.log"))
            .idle_minutes(self.budget.idle_minutes)
            .run()
            .context("failed to spawn the `cursor-agent` CLI (is it installed?)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INIT: &str = r#"{"type":"system","subtype":"init","apiKeySource":"login","cwd":"C:\\Dev\\FinCal","session_id":"868f1553-01ac-4335-89c6-6c1f101d6009","model":"Auto","permissionMode":"force"}"#;

    fn envelope(subtype: &str, is_error: bool, result: &str) -> String {
        serde_json::json!({
            "type": "result",
            "subtype": subtype,
            "is_error": is_error,
            "duration_ms": 25253,
            "result": result,
            "session_id": "868f1553-01ac-4335-89c6-6c1f101d6009",
            "usage": {"inputTokens": 19264, "outputTokens": 1303,
                      "cacheReadTokens": 5248, "cacheWriteTokens": 0}
        })
        .to_string()
    }

    #[test]
    fn clean_success_carries_the_sentinel_as_the_last_line_of_result() {
        let stdout = format!(
            "{INIT}\n{}\n",
            envelope("success", false, "all green\nRALPHY_DONE_EXIT")
        );
        let fold = fold_cursor_stream(&stdout);
        assert!(fold.saw_envelope);
        assert_eq!(fold.subtype.as_deref(), Some("success"));
        assert!(!fold.is_error);
        assert!(
            fold.final_text.ends_with("RALPHY_DONE_EXIT"),
            "{:?}",
            fold.final_text
        );
        assert_eq!(
            fold.session_id.as_deref(),
            Some("868f1553-01ac-4335-89c6-6c1f101d6009")
        );
        assert_eq!(
            classify_cursor_outcome(&fold, true, false, true, Some(0)),
            Outcome::Done
        );
    }

    /// D3 rule 1: the discriminator is inside the tool record, and the run reports
    /// success regardless. The outcome must not read it.
    #[test]
    fn a_failed_tool_call_still_folds_to_a_successful_run() {
        let failed = r#"{"type":"tool_call","subtype":"completed","tool_call":{"shellToolCall":{"args":{"command":"exit 42"},"result":{"failure":{"command":"exit 42","exitCode":42,"signal":null,"aborted":false}}}}}"#;
        let stdout = format!(
            "{INIT}\n{failed}\n{}\n",
            envelope("success", false, "done\nRALPHY_DONE_EXIT")
        );
        let fold = fold_cursor_stream(&stdout);
        assert!(
            !fold.failed_tool_calls.is_empty(),
            "the failure must be recorded"
        );
        assert!(
            fold.failed_tool_calls[0].contains("exit 42"),
            "{:?}",
            fold.failed_tool_calls
        );
        assert!(!fold.is_error, "a failed tool call is not a failed run");
        assert_eq!(
            classify_cursor_outcome(&fold, true, false, true, Some(0)),
            Outcome::Done
        );
        assert!(
            fold.degraded_note().is_some(),
            "but it IS surfaced as degraded"
        );
    }

    /// D7's third discriminator: the operator's deny list wins over `--force`, the
    /// denial is immediate and headless-safe, and the run still reports success.
    #[test]
    fn a_permission_denied_call_is_recorded_and_the_run_still_succeeds() {
        let denied = r#"{"type":"tool_call","subtype":"completed","tool_call":{"shellToolCall":{"result":{"permissionDenied":{"command":"git status --short","workingDirectory":"C:\\Dev\\FinCal","error":"Command blocked by permissions configuration","isReadonly":false}}}}}"#;
        let stdout = format!(
            "{INIT}\n{denied}\n{}\n",
            envelope("success", false, "done\nRALPHY_DONE_EXIT")
        );
        let fold = fold_cursor_stream(&stdout);
        assert_eq!(
            fold.denied_tool_calls,
            vec!["git status --short".to_string()]
        );
        assert_eq!(
            classify_cursor_outcome(&fold, true, false, true, Some(0)),
            Outcome::Done
        );
        let note = fold
            .degraded_note()
            .expect("a blocked command must be visible");
        assert!(note.contains("git status --short"), "{note}");
    }

    /// D3 rule 2: zero records + exit 1. Distinguishable from a dead child by the
    /// record count, which is why the fold tracks "saw anything at all".
    #[test]
    fn zero_records_and_exit_1_is_a_preflight_rejection() {
        let fold = fold_cursor_stream("");
        assert!(!fold.saw_envelope);
        assert!(fold.saw_no_records(), "no record of any kind arrived");
        assert_eq!(
            classify_cursor_outcome(&fold, false, false, false, Some(1)),
            Outcome::Stuck
        );
    }

    /// D3 rule 3: partial records, no envelope, and an EMPTY stderr — the one case
    /// where stderr says nothing at all, so an adapter classifying on stderr alone
    /// sees a silent success. Here the missing envelope is what fails it.
    #[test]
    fn partial_records_with_no_envelope_is_truncation() {
        let stdout = format!("{INIT}\n{{\"type\":\"assistant\",\"message\":{{\"content\":[]}}\n");
        let fold = fold_cursor_stream(&stdout);
        assert!(!fold.saw_envelope, "the run died before the envelope");
        assert!(
            !fold.saw_no_records(),
            "records DID arrive — not a preflight rejection"
        );
        assert_eq!(
            classify_cursor_outcome(&fold, false, false, false, Some(1)),
            Outcome::Stuck
        );
        // Even with a sentinel somehow present, no envelope means not Done.
        let mut with_sentinel = fold_cursor_stream(&stdout);
        with_sentinel.final_text = "RALPHY_DONE_EXIT".into();
        assert_ne!(
            classify_cursor_outcome(&with_sentinel, true, false, true, Some(0)),
            Outcome::Done,
            "absence of the envelope is itself a failure signal (D3)"
        );
    }

    /// Never reproduced, therefore handled defensively (D3): an unknown `subtype`
    /// is NOT success, even alongside a sentinel and a clean exit.
    #[test]
    fn an_unknown_subtype_is_not_success() {
        let stdout = format!(
            "{INIT}\n{}\n",
            envelope("weird", false, "all green\nRALPHY_DONE_EXIT")
        );
        let fold = fold_cursor_stream(&stdout);
        assert_eq!(fold.subtype.as_deref(), Some("weird"));
        assert_ne!(
            classify_cursor_outcome(&fold, true, false, true, Some(0)),
            Outcome::Done
        );

        // `is_error: true` — the other never-reproduced shape — fails the same way.
        let errored = format!(
            "{INIT}\n{}\n",
            envelope("success", true, "all green\nRALPHY_DONE_EXIT")
        );
        assert_ne!(
            classify_cursor_outcome(&fold_cursor_stream(&errored), true, false, true, Some(0)),
            Outcome::Done
        );
    }

    /// ADR-0038: exit 130 is what Ralphy's own budget and idle watchdogs produce.
    /// Reporting that as `Stuck` would blame the agent for a stop Ralphy chose.
    #[test]
    fn an_interrupt_is_not_reported_as_a_crash() {
        let stdout = format!("{INIT}\n");
        let fold = fold_cursor_stream(&stdout);
        let outcome = classify_cursor_outcome(&fold, false, false, false, Some(130));
        assert_eq!(
            outcome,
            Outcome::Timeout,
            "`Aborting operation...` is a stop, not a crash"
        );
        // A hard kill (no exit code at all, empty stderr) stays Stuck.
        assert_eq!(
            classify_cursor_outcome(&fold, false, false, false, None),
            Outcome::Stuck
        );
    }

    #[test]
    fn a_clean_run_has_no_degraded_note() {
        let stdout = format!(
            "{INIT}\n{}\n",
            envelope("success", false, "RALPHY_DONE_EXIT")
        );
        assert!(fold_cursor_stream(&stdout).degraded_note().is_none());
    }

    /// The WIRING half of D6, and the invariant the whole slice exists for: no test
    /// here spawns a real child, so deleting the gate call would keep the suite
    /// green and turn the refusal into a no-op. Pin the call AND its position — a
    /// gate that runs after the spawn has already uploaded the repository.
    /// Fragments are assembled with `concat!` so the assertion cannot match itself.
    #[test]
    fn the_gate_runs_before_any_child_is_spawned() {
        let src = include_str!("outcome.rs");
        let gate = concat!("indexing_gate(", "work_dir, self.allow_indexing)?;");
        let seed = concat!("seed_cursor_config", "_dir(");
        let spawn = concat!("HeadlessCall::", "new(cmd,");
        let at_gate = src
            .find(gate)
            .expect("run_cursor must call the indexing gate");
        let at_seed = src.find(seed).expect("run_cursor must seed the config dir");
        let at_spawn = src.find(spawn).expect("the HeadlessCall site moved");
        assert!(
            at_gate < at_spawn,
            "D6 must refuse BEFORE the child is spawned, not after"
        );
        assert!(
            at_seed < at_spawn,
            "D17's isolation must be seeded BEFORE the child is spawned"
        );
        assert_eq!(
            src.matches(spawn).count(),
            1,
            "this is the crate's single HeadlessCall site (ADR-0040 Tier 1)"
        );
    }

    /// `shellToolCall.result.success` carries no file-change data at all, so a
    /// progress number read from the stream would report zero for shell work. The
    /// production half must never name those fields.
    #[test]
    fn no_progress_read_from_the_stream() {
        let production = include_str!("outcome.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for banned in ["linesAdded", "linesRemoved", "diffString"] {
            assert!(
                !production.contains(banned),
                "progress comes from the HEAD-diff, never the stream; found {banned}"
            );
        }
    }
}
