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
mod tests;
