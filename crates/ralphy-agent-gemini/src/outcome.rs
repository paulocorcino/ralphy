//! Folding Gemini's `--output-format stream-json` record stream into the signals
//! the shared [`classify`](ralphy_adapter_support::classify) ladder needs
//! (ADR-0023), the exit-code taxonomy (ADR-0043 D3), and the crate's single
//! child-spawning seam.
//!
//! The fold's load-bearing property is that it **joins consecutive assistant
//! `message` records before matching anything**: this vendor emits the final text
//! as a sequence of deltas, so `RALPHY_DONE_EXIT` routinely arrives split across
//! two records. A per-record sentinel match reports a finished session as stuck.

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_adapter_support::{CompletionSignals, HeadlessCall, HeadlessRun};
use ralphy_core::Outcome;
use serde_json::Value;

use crate::GeminiAgent;

/// What one call's stdout reduces to.
#[derive(Debug, Default)]
pub(crate) struct GeminiFold {
    /// Every assistant `message` record joined in arrival order — the sentinel is
    /// matched against THIS, never a single record.
    pub(crate) final_text: String,
    /// `init.session_id`, when the stream carried one.
    pub(crate) session_id: Option<String>,
    /// The model the vendor reported actually using.
    pub(crate) model: Option<String>,
    /// `result.status`. `None` when the terminal record never arrived — which is
    /// a signal in its own right, not a neutral absence.
    pub(crate) status: Option<String>,
    /// Whether the terminal `result` record arrived at all.
    pub(crate) saw_result: bool,
    /// The vendor's own sentence for why it stopped.
    pub(crate) vendor_error: Option<String>,
}

/// Pull the human-readable text out of a record's `content`, which the vendor
/// emits either as a bare string or as an array of typed parts.
fn record_text(obj: &Value) -> String {
    match obj.get("content").or_else(|| obj.get("text")) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                p.as_str()
                    .map(str::to_string)
                    .or_else(|| p.get("text").and_then(Value::as_str).map(str::to_string))
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Reduce one call's stdout to a [`GeminiFold`].
///
/// Tolerant by construction: non-JSON lines are skipped (the CLI interleaves
/// human-readable notices), and a stream that ends without its terminal record is
/// folded as far as it got, with `saw_result` false.
pub(crate) fn fold_gemini_stream(stdout: &str) -> GeminiFold {
    let mut fold = GeminiFold::default();
    for obj in stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
    {
        let kind = obj.get("type").and_then(Value::as_str).unwrap_or_default();
        let role = obj.get("role").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "init" | "system" => {
                if let Some(id) = obj.get("session_id").and_then(Value::as_str) {
                    fold.session_id = Some(id.to_string());
                }
                if let Some(m) = obj.get("model").and_then(Value::as_str) {
                    fold.model = Some(m.to_string());
                }
            }
            // The delta join: append, never match. A sentinel split after `RAL`
            // is only recoverable because the pieces are concatenated first.
            "message" if role == "assistant" || role.is_empty() => {
                fold.final_text.push_str(&record_text(&obj));
            }
            "result" => {
                fold.saw_result = true;
                if let Some(s) = obj.get("status").and_then(Value::as_str) {
                    fold.status = Some(s.to_string());
                }
                if let Some(m) = obj.get("model").and_then(Value::as_str) {
                    fold.model = Some(m.to_string());
                }
                for key in ["error", "message"] {
                    if let Some(e) = obj.get(key).and_then(Value::as_str) {
                        fold.vendor_error = Some(e.to_string());
                        break;
                    }
                }
            }
            "error" => {
                for key in ["error", "message"] {
                    if let Some(e) = obj.get(key).and_then(Value::as_str) {
                        fold.vendor_error = Some(e.to_string());
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    fold
}

/// The vendor's documented exit-code taxonomy (ADR-0043 D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitClass {
    Success,
    Generic,
    Auth,
    BadArgv,
    Sandbox,
    Config,
    TurnLimit,
    ToolFailure,
    Untrusted,
    Cancelled,
    Limit,
    /// A code the taxonomy does not assign. **Not** an error to reach: the CLI's
    /// `extractErrorCode()` forwards any numeric `.code`/`.status` it finds
    /// straight to `process.exit()`, so an upstream HTTP status is reachable here.
    Other,
}

/// Classify the child's exit code. Total by construction — see [`ExitClass::Other`].
pub(crate) fn classify_exit(code: Option<i32>) -> ExitClass {
    match code {
        Some(0) => ExitClass::Success,
        Some(1) => ExitClass::Generic,
        Some(41) => ExitClass::Auth,
        Some(42) => ExitClass::BadArgv,
        Some(44) => ExitClass::Sandbox,
        Some(52) => ExitClass::Config,
        Some(53) => ExitClass::TurnLimit,
        Some(54) => ExitClass::ToolFailure,
        Some(55) => ExitClass::Untrusted,
        Some(130) => ExitClass::Cancelled,
        Some(429) => ExitClass::Limit,
        _ => ExitClass::Other,
    }
}

/// Extract Gemini's [`CompletionSignals`] and delegate the precedence ordering to
/// the shared ladder (ADR-0023 D1/D2).
///
/// **The exit code takes precedence over the envelope**: the stream can carry a
/// `result` record and still exit non-zero (a tool failure, a turn-limit stop),
/// and in that direction the code is the vendor's final word. A run is `errored`
/// unless the code says success AND the envelope arrived saying so — the
/// pessimistic direction, because an unreproduced status must not be assumed
/// benign.
pub(crate) fn classify_gemini_outcome(
    fold: &GeminiFold,
    exited_cleanly: bool,
    timed_out: bool,
    committed: bool,
    exit_code: Option<i32>,
) -> Outcome {
    let class = classify_exit(exit_code);
    let cancelled = class == ExitClass::Cancelled;
    let succeeded =
        class == ExitClass::Success && fold.saw_result && fold.status.as_deref() != Some("error");
    ralphy_adapter_support::classify(CompletionSignals {
        done: ralphy_adapter_support::done_sentinel(&fold.final_text),
        blocked: ralphy_adapter_support::blocked_reason(&fold.final_text),
        // D11 is open: quota exhaustion has never been observed on this vendor, so
        // only the documented rate-limit code claims a limit.
        limit: (class == ExitClass::Limit).then(|| fold.vendor_error.clone()),
        committed,
        // A cancellation IS Ralphy stopping the child, so it lands on `Timeout`
        // rather than falling through the ladder to `Stuck`.
        timed_out: timed_out || cancelled,
        exited_ok: exited_cleanly && !cancelled,
        errored: !succeeded,
    })
}

impl GeminiAgent {
    /// Spawn a single headless `gemini` call, piping `prompt` on stdin and
    /// draining stdout/stderr via the shared headless runner. The crate's single
    /// [`HeadlessCall`] site (ADR-0040 Tier 1).
    ///
    /// **Cross-path invariant:** `root::ensure` and `policy::write_policy` run
    /// BEFORE every spawn, on every path — plan, execute and the login probe —
    /// never once at construction. A child spawned against a root that does not
    /// exist yet falls back to the operator's own, which is precisely the
    /// isolation D4 exists to guarantee.
    ///
    /// The prompt is fully built by the caller before this is reached: the vendor
    /// gives stdin a 500 ms grace timer after spawn, and `HeadlessCall` writes the
    /// payload it was constructed with immediately (see
    /// `the_prompt_is_computed_before_the_child_is_spawned`).
    pub(crate) fn run_gemini(
        &self,
        cmd: Command,
        prompt: &str,
        timeout: Duration,
    ) -> Result<HeadlessRun> {
        HeadlessCall::new(cmd, prompt, timeout, &self.run_dir.join("gemini.log"))
            .idle_minutes(self.budget.idle_minutes)
            .run()
            .context("failed to spawn the `gemini` CLI (is it installed?)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live charter round-trip (step 18 of the plan for #253): the assembled
    /// planning charter piped on stdin with markers planted on its first and last
    /// line, plus an argv prompt marker.
    const CHARTER_ROUNDTRIP: &str = include_str!("../fixtures/charter-roundtrip-2026-07-21.jsonl");

    fn msg(role: &str, content: &str) -> String {
        serde_json::json!({"type": "message", "role": role, "content": content}).to_string()
    }

    /// The defect this exists to catch: the sentinel arrives SPLIT across two
    /// delta records, so a per-record match reports a finished session as stuck.
    #[test]
    fn fold_joins_a_sentinel_split_across_delta_records() {
        let stdout = format!(
            "{}\n{}\n{}\n",
            msg("assistant", "all green\nRAL"),
            msg("assistant", "PHY_DONE_EXIT"),
            serde_json::json!({"type": "result", "status": "success"})
        );
        let fold = fold_gemini_stream(&stdout);
        assert!(
            ralphy_adapter_support::done_sentinel(&fold.final_text),
            "the joined text must carry the sentinel: {:?}",
            fold.final_text
        );
        // The discriminating control: matching per record finds nothing.
        assert!(!ralphy_adapter_support::done_sentinel("all green\nRAL"));
        assert!(!ralphy_adapter_support::done_sentinel("PHY_DONE_EXIT"));
        assert!(fold.saw_result);
        assert_eq!(fold.status.as_deref(), Some("success"));
    }

    /// A pre-flight failure ends the stream with no terminal record at all; the
    /// fold must still classify rather than panic or claim success.
    #[test]
    fn a_missing_result_record_is_still_classified() {
        let stdout = format!("{}\n", msg("assistant", "partial work"));
        let fold = fold_gemini_stream(&stdout);
        assert!(!fold.saw_result);
        assert_eq!(fold.status, None);
        // Zero records at all — a rejection before the model was ever reached.
        let empty = fold_gemini_stream("Error: something went wrong\n");
        assert!(!empty.saw_result);
        assert!(empty.final_text.is_empty());
        // Neither is reported as a green run.
        for f in [&fold, &empty] {
            assert_ne!(
                classify_gemini_outcome(f, false, false, false, Some(1)),
                Outcome::Done
            );
        }
    }

    /// A non-ASCII charter — including an astral-plane character — must survive
    /// the fold byte-exact. A fold that sliced on `char` boundaries or re-encoded
    /// would corrupt exactly this payload.
    #[test]
    fn a_non_ascii_charter_survives_the_fold() {
        const PAYLOAD: &str = "𝄞 café 日本語 — ✅";
        let stdout = format!(
            "{}\n{}\n",
            msg("assistant", PAYLOAD),
            serde_json::json!({"type": "result", "status": "success"})
        );
        let fold = fold_gemini_stream(&stdout);
        assert_eq!(fold.final_text, PAYLOAD);
        assert_eq!(fold.final_text.as_bytes(), PAYLOAD.as_bytes());
        // Split across deltas mid-payload, the join must still be byte-exact.
        let (a, b) = PAYLOAD.split_at("𝄞 café ".len());
        let split = format!("{}\n{}\n", msg("assistant", a), msg("assistant", b));
        assert_eq!(fold_gemini_stream(&split).final_text, PAYLOAD);
    }

    /// D3's table, plus the two codes that prove it is not a closed set: `429`
    /// (reachable because `extractErrorCode()` forwards any numeric `.code`) and
    /// an unassigned number.
    #[test]
    fn classify_exit_maps_the_taxonomy_and_an_unknown_code() {
        for (code, want) in [
            (Some(0), ExitClass::Success),
            (Some(1), ExitClass::Generic),
            (Some(41), ExitClass::Auth),
            (Some(42), ExitClass::BadArgv),
            (Some(44), ExitClass::Sandbox),
            (Some(52), ExitClass::Config),
            (Some(53), ExitClass::TurnLimit),
            (Some(54), ExitClass::ToolFailure),
            (Some(55), ExitClass::Untrusted),
            (Some(130), ExitClass::Cancelled),
            (Some(429), ExitClass::Limit),
            (Some(999), ExitClass::Other),
            (None, ExitClass::Other),
        ] {
            assert_eq!(classify_exit(code), want, "exit {code:?}");
        }
    }

    /// The exit code outranks the envelope: a stream that reported success while
    /// the process exited on a tool failure is not a green run.
    #[test]
    fn the_exit_code_outranks_the_envelope() {
        let stdout = format!(
            "{}\n{}\n",
            msg("assistant", "done\nRALPHY_DONE_EXIT"),
            serde_json::json!({"type": "result", "status": "success"})
        );
        let fold = fold_gemini_stream(&stdout);
        assert_eq!(
            classify_gemini_outcome(&fold, true, false, true, Some(0)),
            Outcome::Done
        );
        assert_ne!(
            classify_gemini_outcome(&fold, false, false, true, Some(54)),
            Outcome::Done,
            "exit 54 (tool failure) must not be reported as a completed run"
        );
        // A cancellation is Ralphy stopping the child, not a crash.
        assert_eq!(
            classify_gemini_outcome(&fold, false, false, true, Some(130)),
            Outcome::Timeout
        );
    }

    /// D2.2: the vendor gives stdin a 500 ms grace timer after spawn, so the whole
    /// prompt must exist BEFORE the child is created. Pinned on the source rather
    /// than assumed from the API shape — `HeadlessCall::new` takes the payload by
    /// value, and `crates/ralphy-adapter-support/src/headless.rs` writes it
    /// immediately after spawning the reader threads (read 2026-07-21).
    #[test]
    fn the_prompt_is_computed_before_the_child_is_spawned() {
        let outcome_src = include_str!("outcome.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert_eq!(
            outcome_src
                .matches(concat!("HeadlessCall::", "new("))
                .count(),
            1,
            "one spawn site in the crate (ADR-0040 Tier 1)"
        );
        // …and it is handed a prompt the caller already owns, never a closure or a
        // reader the child could race.
        assert!(
            outcome_src.contains(concat!("HeadlessCall::", "new(cmd, prompt, timeout,")),
            "the payload must be complete at construction (D2.2's 500 ms grace timer)"
        );
        let lib_src = include_str!("lib.rs");
        assert!(
            !lib_src.contains(concat!("HeadlessCall::", "new(")),
            "lib.rs must go through run_gemini, not spawn its own child"
        );
    }

    /// D2, live: standard input is PREPENDED to the argv prompt and joined with a
    /// blank line — the vendor's documentation states this backwards, and a
    /// charter delivered after the argv word would be read as a trailing note.
    ///
    /// The fixture is one real invocation (2026-07-21, gemini 0.51.0): the
    /// assembled `prompt.plan.gemini.md` piped on stdin with `RALPHY_CHARTER_HEAD_9F2A`
    /// planted on its first line and the non-ASCII payload plus
    /// `RALPHY_CHARTER_TAIL_7B31` on its last, and `-p "RALPHY_ARGV_TAIL_51CD"`.
    #[test]
    fn stdin_arrives_before_the_argv_prompt() {
        let user = CHARTER_ROUNDTRIP
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
            .find(|v| v.get("role").and_then(Value::as_str) == Some("user"))
            .expect("the fixture must carry the user record");
        let text = record_text(&user);

        assert!(
            text.starts_with("RALPHY_CHARTER_HEAD_9F2A"),
            "stdin must come FIRST: {:?}",
            &text[..text.len().min(120)]
        );
        assert!(
            text.ends_with("RALPHY_ARGV_TAIL_51CD"),
            "the argv prompt must come LAST: {:?}",
            &text[text.len().saturating_sub(120)..]
        );
        // Exactly one blank line joins the two, and the astral-plane payload
        // planted just before the stdin tail marker survived the round trip.
        assert!(
            text.contains("𝄞 café 日本語 — ✅ RALPHY_CHARTER_TAIL_7B31\n\nRALPHY_ARGV_TAIL_51CD"),
            "stdin and argv must be joined by exactly one blank line, with the \
             non-ASCII payload intact"
        );
    }

    /// The same fixture proves the argv carried no prompt flag other than the one
    /// marker this probe deliberately planted: everything else the session saw
    /// arrived on stdin.
    #[test]
    fn the_roundtrip_fixture_carries_the_whole_charter() {
        let user = CHARTER_ROUNDTRIP
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
            .find(|v| v.get("role").and_then(Value::as_str) == Some("user"))
            .expect("the fixture must carry the user record");
        let text = record_text(&user);
        assert!(
            text.len() > 23_000,
            "the whole ~24 KB charter must have arrived, got {} bytes",
            text.len()
        );
    }
}
