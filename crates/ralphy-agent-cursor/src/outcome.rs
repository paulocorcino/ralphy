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
    /// Whether a terminal `turn_ended` record arrived. It is a record in its own
    /// right, so a run that ends on one is NOT the zero-record shape of a
    /// preflight rejection, however empty the rest of the fold looks.
    pub(crate) saw_turn_end: bool,
    /// The vendor's own sentence for why it stopped. Two carriers, folded to the
    /// same field: a `turn_ended.error` record (measured 2026-07-21), OR the bare
    /// `ActionRequiredError:` stderr prose ADR-0042 anticipated and the capstone
    /// finally measured (2026-07-22, #251) — a quota stop with NO terminal record
    /// at all. Reading it is what keeps an explicit, self-describing refusal from
    /// degrading to a mute `Stuck`. What Ralphy *does* with a quota stop is #266.
    pub(crate) vendor_error: Option<String>,
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
            && !self.saw_turn_end
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

/// The quota/rate-limit sentence CLASSES Cursor uses, case-insensitive: the
/// vendor's wording is editor-framed marketing prose and varies, so a class match
/// is stabler than the whole sentence (ADR-0040 C7 precedent).
const LIMIT_CLASSES: &[&str] = &[
    "usage limit",
    "rate limit",
    "quota",
    "too many requests",
    "resource exhausted",
];

/// The quota sentence when `line` is the vendor's bare `ActionRequiredError:` prose
/// for a usage/rate limit — the shape that arrives on stderr with NO terminal
/// record (measured live 2026-07-22, #251), which the record path never caught.
/// `None` for a model-entitlement `ActionRequiredError` (that is `model_refusal_stop`'s
/// job) or any other line.
///
/// Line-start gated on [`model::ERROR_CLASS`], exactly as `model_refusal_stop` is:
/// stdout stream-json lines begin with `{`, so a green run's transcript quoting the
/// sentence can never reach this. The returned string omits the class prefix, so it
/// reads identically to the `turn_ended.error` a record-shape stop carries.
fn bare_limit_prose(line: &str) -> Option<String> {
    let rest = line.strip_prefix(crate::model::ERROR_CLASS)?;
    let lower = rest.to_lowercase();
    LIMIT_CLASSES
        .iter()
        .any(|class| lower.contains(class))
        .then(|| rest.to_string())
}

/// Fold one call's stream. Reads `r.stdout` on the paths that only need the JSON
/// records, or the MERGED `r.log` on the paths that must also see a bare-stderr
/// limit (the plan closure and execute's limit check) — folding the merged log is a
/// superset, since every non-JSON line is skipped save the gated one below.
///
/// Lines that do not parse as JSON are skipped, so a truncated last line — the
/// ordinary shape of a killed child — never panics. The one exception is the
/// vendor's bare `ActionRequiredError:` quota line ([`bare_limit_prose`]).
pub(crate) fn fold_cursor_stream(stdout: &str) -> CursorFold {
    let mut fold = CursorFold::default();
    for line in stdout.lines() {
        let trimmed = line.trim();
        let Ok(obj) = serde_json::from_str::<Value>(trimmed) else {
            // Not JSON. Capture only the bare-stderr quota prose (#251); everything
            // else here is genuinely noise. First one wins, so a later stderr line
            // cannot overwrite a `turn_ended.error` already read from a record.
            if let Some(msg) = bare_limit_prose(trimmed) {
                fold.is_error = true;
                fold.vendor_error.get_or_insert(msg);
            }
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
            // The OTHER terminal record. It carries its own verdict in `status`,
            // and folding it without reading that field is how a vendor refusal
            // that names its own cause arrives as a mute stop. Any status other
            // than `success` is an error, in the same pessimistic direction as
            // the envelope's unknown `subtype`.
            ("turn_ended", _) => {
                fold.saw_turn_end = true;
                let status = obj
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if status != "success" {
                    fold.is_error = true;
                    if let Some(msg) = obj.get("error").and_then(Value::as_str) {
                        fold.vendor_error = Some(msg.to_string());
                    }
                }
            }
            // NOT gated on `subtype == "completed"`: the ADR never pins the subtype
            // of the `permissionDenied` record it quotes, and a hardcoded value here
            // would be the same silent-stop-discriminating failure the structural
            // walk below exists to avoid. The walk is a no-op on a record carrying
            // no result discriminator, so scanning every `tool_call` costs nothing.
            ("tool_call", _) => {
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

/// `Some(sentence)` when `fold.vendor_error` names a quota/rate-limit CLASS —
/// case-insensitive `usage limit`, `rate limit`, `quota`, `too many requests`,
/// `resource exhausted` (ADR-0040 C7 precedent; the measured sentence is
/// editor-framed marketing prose and will be reworded). `None` otherwise.
///
/// Reads `vendor_error` ONLY. It is populated from a `turn_ended` record whose
/// `status != "success"`, OR from a bare `ActionRequiredError:` stderr line
/// ([`bare_limit_prose`]) — both gated in `fold_cursor_stream` so a transcript
/// quoting the sentence in a GREEN run's `final_text` can never reach it. A caller
/// that wants the bare-stderr shape too must therefore fold the MERGED log, not
/// `r.stdout` alone (the plan closure and execute's limit check both do).
pub(crate) fn cursor_limit_note(fold: &CursorFold) -> Option<String> {
    let msg = fold.vendor_error.as_deref()?;
    let lower = msg.to_lowercase();
    LIMIT_CLASSES
        .iter()
        .any(|class| lower.contains(class))
        .then(|| msg.to_string())
}

/// The operator-facing note for a quota stop: the vendor's own sentence, plus,
/// when the call already committed real work, the HEAD-diff range so the
/// operator can find it before the issue resumes on ADR-0030's synthetic wait.
pub(crate) fn limit_stop_note(fold: &CursorFold, committed_range: Option<&str>) -> Option<String> {
    let msg = cursor_limit_note(fold)?;
    let mut note = format!("cursor stopped on a usage limit: {msg}");
    if let Some(range) = committed_range {
        note.push_str(&format!(
            " — work already committed ({range}) is kept on the branch for \
             inspection; the issue stays open"
        ));
    }
    Some(note)
}

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
    let limited = !succeeded && cursor_limit_note(fold).is_some();
    ralphy_adapter_support::classify(CompletionSignals {
        done: ralphy_adapter_support::done_sentinel(&fold.final_text),
        blocked: ralphy_adapter_support::blocked_reason(&fold.final_text),
        // D13: `Limit(None)`. The inner slot is the parsed RESET HINT, and this
        // vendor publishes none, so ADR-0030's synthetic cadence applies. The
        // sentence goes to the run log via `limit_stop_note`, not into the slot —
        // putting it there would make `runner/phases.rs` read it as a scheduled
        // reset and abandon the issue after two no-commit limits.
        limit: limited.then_some(None),
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
    /// child spawned before the gate has written the opt-out has already uploaded
    /// the repository, so protecting it afterwards protects nothing.
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

    const PERMISSION_DENIED: &str = include_str!("../fixtures/permission-denied-2026-07-20.jsonl");
    const TOOL_FAILURE: &str = include_str!("../fixtures/tool-failure-2026-07-20.jsonl");
    const KILLED: &str = include_str!("../fixtures/killed-2026-07-20.jsonl");
    const KILLED_ERR: &str = include_str!("../fixtures/killed-2026-07-20.err");
    const INTERRUPTED_STREAM: &str = include_str!("../fixtures/interrupted-2026-07-20.jsonl");
    const INTERRUPTED_ERR: &str = include_str!("../fixtures/interrupted-2026-07-20.err");
    const PREFLIGHT_REJECTION: &str =
        include_str!("../fixtures/preflight-rejection-2026-07-20.jsonl");
    const PREFLIGHT_REJECTION_ERR: &str =
        include_str!("../fixtures/preflight-rejection-2026-07-20.err");
    /// The two quota refusals that blocked the live execute pass on 2026-07-21.
    ///
    /// PROVENANCE, because it is not the same as the fixtures above: these were
    /// recovered from the vendor's session store
    /// (`~/.cursor/projects/<slug>/agent-transcripts/`), the run's stdout having
    /// not been captured. That store serialises the conversational records
    /// differently from the stream — bare `{"role","message"}` objects with no
    /// `type` field — and the fold skips those either way. What both files share
    /// with the stream, byte for byte, is the terminal record under test.
    const USAGE_LIMIT: &str = include_str!("../fixtures/usage-limit-2026-07-21.jsonl");
    const USAGE_LIMIT_MIDTURN: &str =
        include_str!("../fixtures/usage-limit-midturn-2026-07-21.jsonl");
    /// The quota stop AS MEASURED: the vendor's bare `ActionRequiredError:` stderr
    /// prose in the MERGED log, with NO terminal record at all. Captured live on
    /// 2026-07-22 during the #251 capstone — the `cursor.log` of a plan run whose
    /// child hit the Free-tier limit, byte-identical to the direct `agent -p` stderr.
    /// The record-shape fixtures above were D13's pre-implementation guess; THIS is
    /// the shape the run found, and the fold must read it the same way.
    const USAGE_LIMIT_STDERR: &str = include_str!("../fixtures/usage-limit-stderr-2026-07-22.log");

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

    /// D6/D3: the permission-denied fixture's envelope is a genuine `subtype:
    /// "success"` stream whose text carries the spike's decoy token, not Ralphy's
    /// `DONE_SENTINEL` — so committing something is not the same as buying a green
    /// close.
    #[test]
    fn a_permission_denied_run_is_green_and_names_the_blocked_command() {
        let fold = fold_cursor_stream(PERMISSION_DENIED);
        assert_eq!(fold.subtype.as_deref(), Some("success"));
        assert!(!fold.is_error);
        assert_eq!(
            fold.denied_tool_calls,
            vec!["git status --short".to_string()]
        );
        let note = fold
            .degraded_note()
            .expect("a blocked command must be visible");
        assert!(note.contains("git status --short"), "{note}");
    }

    /// Companion to the pin above, over the SAME fixture: a real success envelope
    /// whose text never carries `DONE_SENTINEL` must not classify `Done`, even with
    /// `committed = true` — commits alone never buy a green close.
    #[test]
    fn an_envelope_without_the_sentinel_is_not_done() {
        let fold = fold_cursor_stream(PERMISSION_DENIED);
        assert_ne!(
            classify_cursor_outcome(&fold, true, false, true, Some(0)),
            Outcome::Done,
            "commits alone never buy a green close"
        );
    }

    /// D3 rule 1: the discriminator is inside the tool record, and the run reports
    /// success regardless. The outcome must not read it — proved by clearing the
    /// tool-call vectors and reclassifying to the SAME `Outcome`.
    #[test]
    fn a_failed_tool_call_does_not_change_the_outcome() {
        let mut fold = fold_cursor_stream(TOOL_FAILURE);
        assert!(
            fold.failed_tool_calls[0].contains("exit 42"),
            "{:?}",
            fold.failed_tool_calls
        );
        assert_eq!(fold.subtype.as_deref(), Some("success"));
        assert!(!fold.is_error, "a failed tool call is not a failed run");
        assert!(
            fold.degraded_note().is_some(),
            "but it IS surfaced as degraded"
        );
        let before = classify_cursor_outcome(&fold, true, false, true, Some(0));
        fold.failed_tool_calls.clear();
        fold.denied_tool_calls.clear();
        let after = classify_cursor_outcome(&fold, true, false, true, Some(0));
        assert_eq!(
            before, after,
            "the outcome never reads the tool-call vectors"
        );
    }

    /// D3 rule 3: partial records, no envelope, and an EMPTY stderr — the one case
    /// where stderr says nothing at all, so an adapter classifying on stderr alone
    /// sees a silent success. Here the missing envelope is what fails it.
    #[test]
    fn a_truncated_stream_has_records_but_no_envelope() {
        let fold = fold_cursor_stream(KILLED);
        assert!(!fold.saw_envelope, "the run died before the envelope");
        assert!(
            !fold.saw_no_records(),
            "records DID arrive — not a preflight rejection"
        );
        assert_eq!(
            classify_cursor_outcome(&fold, false, false, false, Some(1)),
            Outcome::Stuck
        );
        assert!(KILLED_ERR.is_empty(), "the empty-stderr half of D3 rule 3");
    }

    /// D3 rule 2: zero records + exit 1. Distinguishable from a truncated run (the
    /// killed fixture) by the record count, which is why the fold tracks "saw
    /// anything at all".
    #[test]
    fn a_preflight_rejection_has_zero_records() {
        let fold = fold_cursor_stream(PREFLIGHT_REJECTION);
        assert!(!fold.saw_envelope);
        assert!(fold.saw_no_records(), "no record of any kind arrived");
        assert!(
            !fold_cursor_stream(KILLED).saw_no_records(),
            "the killed fixture DID see records — that's the discriminator"
        );
        assert_eq!(
            classify_cursor_outcome(&fold, false, false, false, Some(1)),
            Outcome::Stuck
        );
        assert!(
            PREFLIGHT_REJECTION_ERR.contains("Workspace directory does not exist"),
            "{PREFLIGHT_REJECTION_ERR}"
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

    /// A vendor refusal that names its own cause must not arrive as a mute stop.
    /// Measured: the account's allowance ran out and the vendor said so, in a
    /// well-formed terminal record — not in the `ActionRequiredError` stderr prose
    /// ADR-0042 anticipated. Reading `status` is the whole difference between
    /// "Stuck, no idea" and the vendor's own sentence in the run log.
    #[test]
    fn a_turn_ended_in_error_carries_the_vendors_reason() {
        let fold = fold_cursor_stream(USAGE_LIMIT);
        assert!(fold.is_error, "status: error is an error");
        assert!(
            fold.vendor_error
                .as_deref()
                .is_some_and(|m| m.contains("usage limit")),
            "{:?}",
            fold.vendor_error
        );
        assert!(
            !fold.saw_no_records(),
            "the turn ended on a record — this is a refusal, not a preflight rejection"
        );
        assert_ne!(
            classify_cursor_outcome(&fold, false, false, false, Some(1)),
            Outcome::Done
        );
    }

    /// The same refusal arriving mid-turn, after the agent had already merged
    /// `origin/main` through its shell tool. Nothing about the tool work makes the
    /// stop any less explicit, and no `result` envelope ever comes.
    #[test]
    fn a_midturn_refusal_reads_the_same_as_a_bare_one() {
        let fold = fold_cursor_stream(USAGE_LIMIT_MIDTURN);
        assert!(!fold.saw_envelope, "the stream ends on `turn_ended`");
        assert_eq!(
            fold.vendor_error,
            fold_cursor_stream(USAGE_LIMIT).vendor_error,
            "one refusal, one sentence, wherever in the turn it lands"
        );
        // Committed work does not buy a green close for a run the vendor refused
        // to finish: the sentinel never arrived.
        assert_ne!(
            classify_cursor_outcome(&fold, false, false, true, Some(1)),
            Outcome::Done
        );
    }

    /// #266: a quota stop classifies as `Limit(None)` — no reset hint is ever
    /// published, so ADR-0030's synthetic cadence schedules the resumption.
    #[test]
    fn a_quota_stop_is_a_limit_with_no_reset_hint() {
        let fold = fold_cursor_stream(USAGE_LIMIT);
        assert!(
            cursor_limit_note(&fold)
                .as_deref()
                .is_some_and(|m| m.contains("You've hit your usage limit")),
            "{:?}",
            cursor_limit_note(&fold)
        );
        assert_eq!(
            classify_cursor_outcome(&fold, false, false, false, Some(1)),
            Outcome::Limit(None)
        );

        let midturn = fold_cursor_stream(USAGE_LIMIT_MIDTURN);
        assert!(
            cursor_limit_note(&midturn)
                .as_deref()
                .is_some_and(|m| m.contains("You've hit your usage limit")),
            "{:?}",
            cursor_limit_note(&midturn)
        );
        assert_eq!(
            classify_cursor_outcome(&midturn, false, false, true, Some(1)),
            Outcome::Limit(None)
        );
    }

    /// #266: the note names the partial work when the call already committed,
    /// and stays silent about it when nothing landed.
    #[test]
    fn a_midturn_quota_stop_names_the_partial_work() {
        let midturn = fold_cursor_stream(USAGE_LIMIT_MIDTURN);
        let note = limit_stop_note(&midturn, Some("abc1234..def5678"))
            .expect("a quota stop must produce a note");
        assert!(note.contains("abc1234..def5678"), "{note}");
        assert!(note.contains("kept on the branch"), "{note}");

        let bare = fold_cursor_stream(USAGE_LIMIT);
        let note = limit_stop_note(&bare, None).expect("a quota stop must produce a note");
        assert!(note.contains("usage limit"), "{note}");
        assert!(!note.contains("kept on the branch"), "{note}");
    }

    /// #251: the quota stop AS IT ACTUALLY ARRIVES — a bare `ActionRequiredError:`
    /// stderr line, no `turn_ended`, no envelope. Before the capstone this shape
    /// folded to nothing, so the plan path reported "produced no plan" and execute
    /// reported `Stuck`; both buried the vendor's own sentence. It must read as
    /// `Limit(None)`, and carry the SAME sentence as the record shape.
    #[test]
    fn the_bare_stderr_quota_shape_is_a_limit() {
        let fold = fold_cursor_stream(USAGE_LIMIT_STDERR);
        assert!(
            !fold.saw_turn_end && !fold.saw_envelope,
            "the measured shape carries NO terminal record"
        );
        assert!(
            fold.vendor_error
                .as_deref()
                .is_some_and(|m| m.contains("usage limit")),
            "the bare stderr line must carry the vendor's sentence: {:?}",
            fold.vendor_error
        );
        assert!(
            cursor_limit_note(&fold)
                .as_deref()
                .is_some_and(|m| m.contains("You've hit your usage limit")),
            "{:?}",
            cursor_limit_note(&fold)
        );
        assert_eq!(
            fold.vendor_error,
            fold_cursor_stream(USAGE_LIMIT).vendor_error,
            "one refusal, one sentence — bare-stderr and record shapes fold alike"
        );
        assert_eq!(
            classify_cursor_outcome(&fold, false, false, false, Some(1)),
            Outcome::Limit(None)
        );
    }

    /// #251: the bare-line capture is gated to the LIMIT classes. A bare
    /// `ActionRequiredError:` that is NOT a quota — an entitlement refusal, which
    /// `model_refusal_stop` owns — must not fold to a `vendor_error`, or it would
    /// masquerade as a quota stop and schedule a pointless wait.
    #[test]
    fn a_bare_non_limit_action_required_is_not_a_limit() {
        let stream =
            format!("{INIT}\nActionRequiredError: Named models unavailable on your plan\n");
        let fold = fold_cursor_stream(&stream);
        assert_eq!(fold.vendor_error, None, "not a quota class → not captured");
        assert_eq!(cursor_limit_note(&fold), None);
    }

    /// #266: `vendor_error` is the only carrier — a working run whose transcript
    /// merely QUOTES the sentence must not classify as a limit.
    #[test]
    fn a_quoted_quota_sentence_in_a_green_transcript_is_not_a_limit() {
        let stdout = format!(
            "{INIT}\n{}\n",
            envelope(
                "success",
                false,
                "You've hit your usage limit\nRALPHY_DONE_EXIT"
            )
        );
        let fold = fold_cursor_stream(&stdout);
        assert_eq!(cursor_limit_note(&fold), None);
        assert_eq!(
            classify_cursor_outcome(&fold, true, false, true, Some(0)),
            Outcome::Done
        );
    }

    /// #266: a quota stop is distinct from #245's entitlement refusal and from
    /// Ralphy's own budget/idle-watchdog stop.
    #[test]
    fn a_quota_stop_is_not_an_entitlement_refusal_nor_a_watchdog_stop() {
        const ENTITLEMENT: &str = include_str!("../fixtures/model-entitlement-2026-07-21.err");
        assert_eq!(cursor_limit_note(&fold_cursor_stream(ENTITLEMENT)), None);

        let fold = fold_cursor_stream(INTERRUPTED_STREAM);
        assert_eq!(
            classify_cursor_outcome(&fold, false, false, false, Some(130)),
            Outcome::Timeout
        );
    }

    /// #266: the ADR closes D13 — pin that the rewritten section documents the
    /// carrier and drops the "pending" marker. Phrases are kept short so they
    /// cannot straddle the ADR's ~78-col hard wrap (`.ralphy/knowledge/issue-264.md`).
    #[test]
    fn the_limit_stance_is_documented() {
        let adr = include_str!("../../../docs/adr/0042-cursor-adapter.md");
        assert!(
            !adr.contains("Limits: pending"),
            "D13 must no longer read pending"
        );
        assert!(
            adr.contains("turn_ended"),
            "D13 must name the measured carrier"
        );
        assert!(
            adr.contains("Limit(None)"),
            "D13 must name the classified outcome"
        );
    }

    /// A `turn_ended` that says `success` is not an error, and — since the ladder
    /// keys success off the `result` envelope — it does not manufacture one either.
    #[test]
    fn a_successful_turn_end_is_not_an_error() {
        let stdout = r#"{"type":"turn_ended","status":"success"}"#;
        let fold = fold_cursor_stream(stdout);
        assert!(!fold.is_error);
        assert_eq!(fold.vendor_error, None);
        assert!(!fold.saw_envelope);
        assert_ne!(
            classify_cursor_outcome(&fold, true, false, true, Some(0)),
            Outcome::Done,
            "a turn end without the envelope is still a truncated stream"
        );
    }

    /// ADR-0038: exit 130 is what Ralphy's own budget and idle watchdogs produce.
    /// Reporting that as `Stuck` would blame the agent for a stop Ralphy chose.
    #[test]
    fn an_interrupt_is_reported_as_interrupted() {
        let fold = fold_cursor_stream(INTERRUPTED_STREAM);
        let outcome = classify_cursor_outcome(&fold, false, false, false, Some(130));
        assert_eq!(
            outcome,
            Outcome::Timeout,
            "`Aborting operation...` is a stop, not a crash"
        );
        assert_eq!(
            INTERRUPTED_ERR.trim(),
            "Aborting operation...",
            "{INTERRUPTED_ERR:?}"
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
    /// green and let a run spawn before the opt-out is written. Pin the call AND its
    /// position — a gate that runs after the spawn has already uploaded the
    /// repository. Fragments are assembled with `concat!` so the assertion cannot
    /// match itself.
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
            "D6 must write the opt-out BEFORE the child is spawned, not after"
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

    /// The pin above counts spawns in ONE file, which is structurally blind to a
    /// child spawned from another module — and one exists (`auth::probe_cursor_login`).
    /// So enumerate every spawn site in the crate and assert each one is accounted
    /// for: either it is gated (the run path) or it runs in a throwaway cwd and
    /// config dir, where D6 has nothing to refuse and D17 nothing to protect.
    /// Recursive, so an ADR-0022 `foo.rs` + `foo/` split cannot silently drop a file.
    #[test]
    fn every_spawn_site_in_the_crate_is_gated_or_neutralized() {
        fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).expect("readable src dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    sources(&path, out);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    let body = std::fs::read_to_string(&path).expect("read source");
                    let production = body.split("#[cfg(test)]").next().unwrap_or("").to_string();
                    out.push((path.display().to_string(), production));
                }
            }
        }
        let mut files = Vec::new();
        sources(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
            &mut files,
        );

        let ctor = concat!("Command::", "new(");
        let spawners = [
            concat!("HeadlessCall::", "new("),
            concat!("run_", "headless("),
            // The one-shots spawn through the shared harness, not `HeadlessCall`
            // directly — without these the whole of `tasks.rs` is invisible here.
            // `run_json_session` is the harness' third public entrypoint (the one
            // the Claude adapter uses); it is listed so a future file reaching for
            // it cannot slip past this scan.
            concat!("run_init_", "session("),
            concat!("run_text_", "session("),
            concat!("run_json_", "session("),
        ];
        let mut sites: Vec<String> = Vec::new();
        for (name, body) in &files {
            if body.contains(ctor) || spawners.iter().any(|s| body.contains(s)) {
                sites.push(name.clone());
            }
        }
        sites.sort();
        let short: Vec<&str> = sites
            .iter()
            .map(|s| s.rsplit(['\\', '/']).next().unwrap_or(s))
            .collect();
        assert_eq!(
            short,
            vec!["auth.rs", "command.rs", "outcome.rs", "tasks.rs"],
            "a NEW child-spawning file appeared — decide its D6/D17/D18 stance and \
             extend this test; a spawn that skips them is the failure this slice exists to prevent"
        );

        // D6/D17 must cover EVERY one-shot spawn — stated as a RELATION, not a
        // literal count: a legitimate fifth verb must not red this, and a preflight
        // moved BELOW its spawn must. Pairs each spawn with the preflight that
        // precedes it, the same positional shape as the run path's pin above.
        let tasks = &files
            .iter()
            .find(|(n, _)| n.ends_with("tasks.rs"))
            .expect("tasks.rs")
            .1;
        let preflight = concat!("one_shot_", "preflight(");
        // Skip the fn's own definition; what remains are the call sites.
        let calls: Vec<usize> = tasks
            .match_indices(preflight)
            .map(|(i, _)| i)
            .skip(1)
            .collect();
        let mut spawns: Vec<usize> = spawners
            .iter()
            .flat_map(|s| tasks.match_indices(s).map(|(i, _)| i))
            .collect();
        spawns.sort_unstable();
        assert!(!spawns.is_empty(), "tasks.rs must hold the one-shot spawns");
        assert_eq!(
            calls.len(),
            spawns.len(),
            "every one-shot spawn needs its own gate+seed call"
        );
        for (call, spawn) in calls.iter().zip(&spawns) {
            assert!(
                call < spawn,
                "a one-shot spawns at byte {spawn} before its preflight at {call}"
            );
        }

        // `command.rs` only BUILDS the command; the run path's gate is pinned above.
        let auth = &files
            .iter()
            .find(|(n, _)| n.ends_with("auth.rs"))
            .expect("auth.rs")
            .1;
        assert!(
            auth.contains(concat!("current_", "dir(scratch.path())")),
            "the login probe must run OUTSIDE the operator's repository (D6)"
        );
        for key in ["CURSOR_CONFIG_DIR", "CURSOR_AGENT_DISABLE_DEBUG_LOG"] {
            assert!(
                auth.contains(key),
                "the login probe must set {key} like every other invocation (D17/D18)"
            );
        }
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

    /// Every committed fixture must be read by a test via `include_str!`, not
    /// re-inlined as a literal record — the fixture rule this whole slice exists
    /// to enforce. Recursive over `src/` so a future ADR-0022 split cannot hide a
    /// fixture from the check.
    #[test]
    fn every_fixture_is_read_by_a_test() {
        fn sources(dir: &Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("readable src dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    sources(&path, out);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    out.push(std::fs::read_to_string(&path).expect("read source"));
                }
            }
        }
        let mut sources_text = Vec::new();
        sources(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
            &mut sources_text,
        );

        let fixtures_dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures"));
        for entry in std::fs::read_dir(fixtures_dir).expect("readable fixtures dir") {
            let path = entry.expect("entry").path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("fixture file name");
            let needle = format!(concat!("include_str!(", "\"../fixtures/{}\")"), name);
            assert!(
                sources_text.iter().any(|src| src.contains(&needle)),
                "fixture {name} is committed but no test reads it via {needle}"
            );
        }

        let result_literal = concat!("\"type\"", ": \"result\"");
        let outcome_src = include_str!("outcome.rs");
        let test_half = outcome_src
            .split("#[cfg(test)]")
            .nth(1)
            .expect("this file has a test module");
        assert!(
            test_half.matches(result_literal).count() <= 2,
            "a new record shape must be captured as a fixture, not inlined — \
             the two never-reproduced synthetic tests are the only exception"
        );
    }
}
