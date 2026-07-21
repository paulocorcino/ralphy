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

/// The vendor's own sentence for why it stopped, from whichever shape a record
/// carries it.
///
/// `error` is an OBJECT on the wire, not a string:
/// `{"type":"result","status":"error","error":{"type":"unknown","message":"[API Error…]"}}`
/// and `{"session_id":…,"error":{"type":"Error","message":"Please set an Auth
/// method…","code":41}}` (spike §, records observed 2026-07-20). A bare
/// `as_str()` on it is `None`, which is how this reduced to a mute stop.
fn record_error(obj: &Value) -> Option<String> {
    let e = obj.get("error")?;
    if let Some(s) = e.as_str() {
        return Some(s.to_string());
    }
    let msg = e.get("message").and_then(Value::as_str)?;
    match e.get("type").and_then(Value::as_str) {
        Some(t) if !t.is_empty() => Some(format!("{t}: {msg}")),
        _ => Some(msg.to_string()),
    }
}

/// The phrases that mean "the provider throttled or exhausted the account"
/// (ADR-0043 D11). Matched over the COMBINED log, because this vendor reserves no
/// exit code for quota: without this a real exhaustion arrives as a mute `Stuck`
/// and the queue burns its no-progress budget on an account-wide throttle.
///
/// Substring matching over a lowercased haystack rather than a regex — the four
/// phrases carry no alternation a regex would buy.
pub(crate) fn gemini_limit_note(text: &str) -> Option<String> {
    let hay = text.to_ascii_lowercase();
    [
        "rate limit",
        "quota exceeded",
        "too many requests",
        "resource exhausted",
    ]
    .into_iter()
    .find(|p| hay.contains(p))
    .map(|p| format!("gemini reported a provider limit ({p})"))
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
            //
            // The role gate is exact: `PROMPT_EXECUTE` itself contains
            // `RALPHY_DONE_EXIT`, and the vendor echoes it back as the
            // `role:"user"` record, so folding anything but the assistant's own
            // words would report every execute call as instantly done.
            "message" if role == "assistant" => {
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
            }
            _ => {}
        }
        // Independent of `type`: the auth-failure record the spike captured
        // carries `error` with NO `type` field at all, so a type-keyed arm drops
        // exactly the record whose sentence the operator needs.
        if fold.vendor_error.is_none() {
            fold.vendor_error = record_error(&obj);
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
    /// The CLI's internal self-relaunch sentinel (ADR-0043 D3/D18). Reaching a
    /// caller means the wrapper's re-exec did NOT complete — a distinct diagnosis
    /// from an unmapped upstream HTTP code, which is why it is not [`Self::Other`].
    Relaunch,
    /// A code the taxonomy does not assign. **Not** an error to reach: the CLI's
    /// `extractErrorCode()` forwards any numeric `.code`/`.status` it finds
    /// straight to `process.exit()`, so an upstream HTTP status is reachable here.
    Other,
}

impl ExitClass {
    /// The operator-facing sentence for an exit that is ACTIONABLE rather than
    /// merely failed (ADR-0043 D5's "detected, not worked around").
    ///
    /// Without this, an enterprise Strict Mode that stripped `--approval-mode
    /// yolo`, a sandbox the host cannot start, and a malformed policy document
    /// all collapse into an unexplained `Stuck` — indistinguishable from a
    /// confused agent, and the one shape a human could fix in a minute.
    pub(crate) fn actionable_stop(self) -> Option<&'static str> {
        match self {
            ExitClass::Untrusted => Some(
                "gemini refused the workspace as untrusted (exit 55) — an admin \
                 policy or enterprise Strict Mode is overriding `--skip-trust`",
            ),
            ExitClass::Sandbox => Some(
                "gemini could not start its sandbox (exit 44) — ralphy sets no \
                 sandbox mode, so this comes from the operator's own settings",
            ),
            ExitClass::Config => Some(
                "gemini rejected its configuration (exit 52) — check ralphy's \
                 owned root under `.ralphy/gemini-home/.gemini/`",
            ),
            ExitClass::BadArgv => Some(
                "gemini rejected the command line (exit 42) — the installed CLI \
                 does not accept the argv this adapter builds",
            ),
            // A budget stop, not a crash: the session ended because it ran out of
            // turns, and reporting it as an unexplained `Stuck` hides the one fact
            // that tells an operator to raise the ceiling rather than debug a hang.
            ExitClass::TurnLimit => Some(
                "gemini stopped at its turn ceiling (exit 53) — a budget stop, not \
                 a crash",
            ),
            ExitClass::ToolFailure => Some(
                "gemini failed executing a tool (exit 54) — the failure is in the \
                 workspace, not in the model",
            ),
            ExitClass::Relaunch => Some(
                "gemini exited on its internal relaunch sentinel (exit 199) — the \
                 CLI's self re-exec did not complete",
            ),
            _ => None,
        }
    }
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
        Some(199) => ExitClass::Relaunch,
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
///
/// `log` is the child's stdout+stderr COMBINED: under `stream-json` the
/// actionable diagnosis (a model-not-found error, the auth sentence, a provider
/// throttle) goes to **stderr** while stdout carries only records, so a
/// classifier reading stdout alone is blind to exactly the failures it must name.
pub(crate) fn classify_gemini_outcome(
    fold: &GeminiFold,
    log: &str,
    exited_cleanly: bool,
    timed_out: bool,
    committed: bool,
    exit_code: Option<i32>,
) -> Outcome {
    let class = classify_exit(exit_code);
    let cancelled = class == ExitClass::Cancelled;
    let succeeded =
        class == ExitClass::Success && fold.saw_result && fold.status.as_deref() != Some("error");
    // This vendor reserves NO exit code for quota (D11), so the text is the only
    // signal a real exhaustion has; `429` alone would never fire.
    let limited = class == ExitClass::Limit || gemini_limit_note(log).is_some();
    ralphy_adapter_support::classify(CompletionSignals {
        done: ralphy_adapter_support::done_sentinel(&fold.final_text),
        blocked: ralphy_adapter_support::blocked_reason(&fold.final_text).or_else(|| {
            // D5: an actionable refusal is a NAMED stop, never a silent
            // degradation into `Stuck`.
            (!succeeded)
                .then(|| class.actionable_stop())
                .flatten()
                .map(str::to_string)
        }),
        // D11: `Limit(None)`. The inner slot is the parsed RESET HINT
        // (`CompletionSignals::limit`), and this vendor publishes none — so
        // ADR-0030's synthetic cadence applies. Putting the vendor's prose here
        // would make `runner/phases.rs` read it as a scheduled reset and abandon
        // the issue after two no-commit limits. The sentence goes to
        // `note_vendor_error` instead.
        limit: limited.then_some(None),
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
    /// BEFORE every spawn on both TURN-DRIVING paths — `plan` and `execute` —
    /// never once at construction. A child spawned against a root that does not
    /// exist yet falls back to the operator's own, which is precisely the
    /// isolation D4 exists to guarantee.
    ///
    /// The login probe (`auth::probe_gemini_login`) is deliberately weaker: it
    /// calls `root::ensure` but carries no policy document, because its argv
    /// (`--list-sessions`) grants no tool and makes no model call — there is
    /// nothing for a policy to deny. The D4 containment it does need is the
    /// `GEMINI_CLI_HOME` it sets.
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
                classify_gemini_outcome(f, "", false, false, false, Some(1)),
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
            (Some(199), ExitClass::Relaunch),
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
            classify_gemini_outcome(&fold, "", true, false, true, Some(0)),
            Outcome::Done
        );
        assert_ne!(
            classify_gemini_outcome(&fold, "", false, false, true, Some(54)),
            Outcome::Done,
            "exit 54 (tool failure) must not be reported as a completed run"
        );
        // A cancellation is Ralphy stopping the child, not a crash.
        assert_eq!(
            classify_gemini_outcome(&fold, "", false, false, true, Some(130)),
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

    /// The vendor writes `error` as an OBJECT, in two shapes — one under a
    /// `result` record, one with NO `type` field at all. A fold that read it as a
    /// string, or that keyed on `type`, dropped both and the stop went mute.
    #[test]
    fn the_vendor_error_object_is_read_in_both_observed_shapes() {
        let under_result = r#"{"type":"result","status":"error","error":{"type":"unknown","message":"[API Error: quota]"}}"#;
        let fold = fold_gemini_stream(under_result);
        assert_eq!(
            fold.vendor_error.as_deref(),
            Some("unknown: [API Error: quota]"),
            "the `error` object under a `result` record must be read"
        );
        assert!(fold.saw_result);

        // The auth record: no `type` key whatsoever.
        let typeless = r#"{"session_id":"s1","error":{"type":"Error","message":"Please set an Auth method in your settings.json","code":41}}"#;
        let fold = fold_gemini_stream(typeless);
        assert_eq!(
            fold.vendor_error.as_deref(),
            Some("Error: Please set an Auth method in your settings.json"),
            "a typeless record carrying `error` must not be dropped"
        );
        // A bare string `error` still works, and a record with none is silent.
        assert_eq!(
            fold_gemini_stream(r#"{"type":"error","error":"boom"}"#)
                .vendor_error
                .as_deref(),
            Some("boom")
        );
        assert_eq!(
            fold_gemini_stream(r#"{"type":"result"}"#).vendor_error,
            None
        );
    }

    /// The role gate must be EXACT. `PROMPT_EXECUTE` itself contains the sentinel
    /// and the vendor echoes the whole prompt back as the `role:"user"` record —
    /// folding anything but the assistant's own words reports every execute call
    /// as instantly done.
    #[test]
    fn the_echoed_user_prompt_never_counts_as_the_agents_answer() {
        const SENTINEL_PROMPT: &str = "…do the work then print RALPHY_DONE_EXIT";
        let terminal = serde_json::json!({"type": "result", "status": "success"});
        // Both discriminating shapes: the echoed `role:"user"` record, and a
        // ROLE-LESS `message` record — a fold that widened to `role.is_empty()`
        // would report the second as the agent's own answer.
        let roleless =
            serde_json::json!({"type": "message", "content": SENTINEL_PROMPT}).to_string();
        for stdout in [
            format!("{}\n{terminal}\n", msg("user", SENTINEL_PROMPT)),
            format!("{roleless}\n{terminal}\n"),
        ] {
            let fold = fold_gemini_stream(&stdout);
            assert!(
                fold.final_text.is_empty(),
                "only the assistant's own words are the answer: {:?}",
                fold.final_text
            );
            assert_ne!(
                classify_gemini_outcome(&fold, "", true, false, false, Some(0)),
                Outcome::Done
            );
        }
    }

    /// D11: this vendor reserves no exit code for quota, so the TEXT is the only
    /// signal a real exhaustion has — and the limit must carry `None`, because the
    /// inner slot is a parsed reset hint the vendor never publishes. Putting prose
    /// there makes the runner read it as a schedule and abandon the issue.
    #[test]
    fn a_provider_throttle_is_a_limit_with_no_reset_hint() {
        // The fold MUST carry a vendor sentence: that is the value an
        // implementation would be tempted to smuggle into the reset slot, and a
        // fold without one cannot tell the two apart.
        let fold = fold_gemini_stream(
            r#"{"type":"result","status":"error","error":{"type":"unknown","message":"[API Error: 429 quota exceeded]"}}"#,
        );
        assert!(fold.vendor_error.is_some(), "the fixture must carry prose");
        for phrase in [
            "Error: 429 Too Many Requests",
            "RESOURCE_EXHAUSTED: quota exceeded for this project",
            "you have hit a rate limit",
        ] {
            assert!(gemini_limit_note(phrase).is_some(), "{phrase}");
            assert_eq!(
                classify_gemini_outcome(&fold, phrase, false, false, false, Some(1)),
                Outcome::Limit(None),
                "a textual throttle must be a limit with NO reset hint: {phrase}"
            );
        }
        // The documented 429 exit reaches the same place with no text at all.
        assert_eq!(
            classify_gemini_outcome(&fold, "", false, false, false, Some(429)),
            Outcome::Limit(None)
        );
        // …and ordinary prose is not a limit.
        assert_eq!(gemini_limit_note("everything is fine"), None);
        assert_ne!(
            classify_gemini_outcome(&fold, "everything is fine", false, false, false, Some(1)),
            Outcome::Limit(None)
        );
    }

    /// D5: an actionable refusal is a NAMED stop, never a silent degradation into
    /// `Stuck`. Without this an enterprise Strict Mode that stripped the autonomy
    /// flag is indistinguishable from a confused agent.
    #[test]
    fn an_actionable_exit_stops_with_a_sentence_not_a_mute_stuck() {
        let fold = fold_gemini_stream("");
        for (code, needle) in [
            (55, "untrusted"),
            (44, "sandbox"),
            (52, "configuration"),
            (42, "command line"),
        ] {
            match classify_gemini_outcome(&fold, "", false, false, false, Some(code)) {
                Outcome::Blocked(reason) => assert!(
                    reason.to_ascii_lowercase().contains(needle),
                    "exit {code} must name its cause, got {reason:?}"
                ),
                other => panic!("exit {code} must be a named stop, got {other:?}"),
            }
        }
        // A plain failure keeps falling through the ladder — this must not turn
        // every non-zero exit into a `Blocked`.
        assert!(!matches!(
            classify_gemini_outcome(&fold, "", false, false, false, Some(1)),
            Outcome::Blocked(_)
        ));
        // …and a SUCCESSFUL run never carries a stop sentence.
        let ok = fold_gemini_stream(&format!(
            "{}\n{}\n",
            msg("assistant", "done"),
            serde_json::json!({"type": "result", "status": "success"})
        ));
        assert!(!matches!(
            classify_gemini_outcome(&ok, "", true, false, true, Some(0)),
            Outcome::Blocked(_)
        ));
    }

    /// The two stops that mean "the session ran out of budget" and "a tool broke",
    /// which both reached the operator as a mute `Stuck` before their sentences
    /// existed. A budget stop and a crash call for opposite reactions — raise the
    /// ceiling versus debug the run — so collapsing them is a real loss.
    #[test]
    fn a_turn_ceiling_is_a_budget_stop_not_a_failure() {
        let fold = fold_gemini_stream("");
        for (code, needle) in [(53, "turn ceiling"), (54, "tool")] {
            match classify_gemini_outcome(&fold, "", false, false, false, Some(code)) {
                Outcome::Blocked(reason) => assert!(
                    reason.to_ascii_lowercase().contains(needle),
                    "exit {code} must name {needle:?}, got {reason:?}"
                ),
                other => panic!("exit {code} must be a named stop, got {other:?}"),
            }
        }
        // The discriminating control: exit 1 is the vendor's generic/model failure
        // and must keep falling through the ladder, or every non-zero exit becomes
        // a `Blocked` and the distinction this test buys is worthless.
        assert!(!matches!(
            classify_gemini_outcome(&fold, "", false, false, false, Some(1)),
            Outcome::Blocked(_)
        ));
    }

    /// D18: the `199` sentinel should never be observed, because the CLI's wrapper
    /// re-execs itself. Observing it means that re-exec broke — a diagnosis worth
    /// its own sentence rather than a fold into the unmapped catch-all.
    #[test]
    fn the_relaunch_sentinel_is_mapped() {
        assert_eq!(classify_exit(Some(199)), ExitClass::Relaunch);
        match classify_gemini_outcome(&fold_gemini_stream(""), "", false, false, false, Some(199)) {
            Outcome::Blocked(reason) => assert!(
                reason.contains("199") && reason.to_ascii_lowercase().contains("relaunch"),
                "the sentinel must name itself, got {reason:?}"
            ),
            other => panic!("exit 199 must be a named stop, got {other:?}"),
        }
    }

    /// The `fold.status != Some("error")` half of `succeeded`, which no other test
    /// discriminates: a clean exit code alone must not make a run green when the
    /// terminal envelope says the session errored.
    #[test]
    fn the_envelope_status_is_honoured_when_present() {
        let stream = |status: &str| {
            format!(
                "{}\n{}\n",
                msg("assistant", "work is done\nRALPHY_DONE_EXIT"),
                serde_json::json!({"type": "result", "status": status})
            )
        };
        assert_ne!(
            classify_gemini_outcome(
                &fold_gemini_stream(&stream("error")),
                "",
                true,
                false,
                true,
                Some(0)
            ),
            Outcome::Done,
            "an errored envelope must not be reported as a completed run"
        );
        // Same stream, same clean exit — only the status differs, so the assertion
        // above cannot be passing for some unrelated reason.
        assert_eq!(
            classify_gemini_outcome(
                &fold_gemini_stream(&stream("success")),
                "",
                true,
                false,
                true,
                Some(0)
            ),
            Outcome::Done
        );
    }

    /// The vendor prints this preamble on stderr on EVERY run, successful ones
    /// included (spike §"stderr is never empty", 2026-07-20) — note the YOLO line
    /// arrives TWICE. A health check keyed on a non-empty stderr, or a limit
    /// matcher loose enough to catch "not available", would report every healthy
    /// run as degraded.
    #[test]
    fn the_startup_preamble_is_not_a_degraded_run() {
        const PREAMBLE: &str = "Warning: 256-color support not detected. Using a terminal with at least 256-color support is recommended…\n\
             YOLO mode is enabled. All tool calls will be automatically approved.\n\
             YOLO mode is enabled. All tool calls will be automatically approved.\n\
             Ripgrep is not available. Falling back to GrepTool.\n";
        let fold = fold_gemini_stream(&format!(
            "{}\n{}\n",
            msg("assistant", "all green\nRALPHY_DONE_EXIT"),
            serde_json::json!({"type": "result", "status": "success"})
        ));
        assert_eq!(
            classify_gemini_outcome(&fold, PREAMBLE, true, false, true, Some(0)),
            Outcome::Done,
            "the routine preamble must not cost a healthy run its Done"
        );
        assert_eq!(gemini_limit_note(PREAMBLE), None);
        assert!(!crate::auth::is_gemini_auth_error(PREAMBLE));
    }

    /// D-both-channels: under `stream-json` the well-typed error object rides
    /// stdout while the readable prose goes to stderr, so a classifier that reads
    /// either one alone is blind to exactly the failures it must name.
    #[test]
    fn both_channels_feed_the_diagnosis() {
        // (a) stdout only: the typed error object is read into the fold.
        let stdout_only = fold_gemini_stream(
            r#"{"type":"result","status":"error","error":{"type":"unknown","message":"[API Error: An unknown error occurred.]"}}"#,
        );
        let vendor = stdout_only
            .vendor_error
            .as_deref()
            .expect("the typed error object must reach the fold");
        assert!(
            vendor.contains("unknown"),
            "the vendor's own sentence must be preserved, got {vendor:?}"
        );

        // (b) stderr only: stdout carries NO record at all — the shape a
        // pre-provider failure leaves — and the diagnosis has to come from the
        // combined log plus the exit code.
        let empty = fold_gemini_stream("");
        assert!(!empty.saw_result && empty.vendor_error.is_none());
        match classify_gemini_outcome(
            &empty,
            "FatalTurnLimitedError: reached the maximum number of turns\n",
            false,
            false,
            false,
            Some(53),
        ) {
            Outcome::Blocked(reason) => assert!(
                reason.to_ascii_lowercase().contains("turn ceiling"),
                "a stderr-only failure must still be named, got {reason:?}"
            ),
            other => panic!("expected a named stop, got {other:?}"),
        }
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
