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
use ralphy_core::{Outcome, Usage};
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
    /// Per-model token usage parsed from `result.stats` (ADR-0043 D9).
    /// `None` when the terminal record carried no `stats` key at all —
    /// including when it never carried a `result` record. `Some` whenever a
    /// `stats` key was present, ALWAYS with at least one item — `usage::
    /// parse_stream_stats` never returns an empty `Vec` — so the split is
    /// "no usage figures at all" vs. "a usage figure, possibly zero-valued",
    /// never a genuinely empty `Some(vec![])`.
    pub(crate) usage: Option<Vec<Usage>>,
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
                if let Some(stats) = obj.get("stats") {
                    fold.usage = Some(crate::usage::parse_stream_stats(stats));
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
    model: Option<&str>,
) -> Outcome {
    let class = classify_exit(exit_code);
    let cancelled = class == ExitClass::Cancelled;
    let succeeded =
        class == ExitClass::Success && fold.saw_result && fold.status.as_deref() != Some("error");
    // This vendor reserves NO exit code for quota (D11), so the text is the only
    // signal a real exhaustion has; `429` alone would never fire.
    //
    // Gated on `!succeeded`: the CLI's own `retryWithBackoff` absorbs transient
    // 429s, so a banner in the log of a run that still went green is the
    // vendor's retry, not an exhaustion — and `limit` outranks `done` in
    // `ralphy_adapter_support::classify`, so an ungated match parks a finished
    // run for ~30 min (#264). Exit `429` stays ungated: it can never coexist
    // with `succeeded`, so gating it would cost nothing but add a distinction
    // with no observable difference.
    let limited = class == ExitClass::Limit || (!succeeded && gemini_limit_note(log).is_some());
    ralphy_adapter_support::classify(CompletionSignals {
        done: ralphy_adapter_support::done_sentinel(&fold.final_text),
        blocked: ralphy_adapter_support::blocked_reason(&fold.final_text).or_else(|| {
            // D5: an actionable refusal is a NAMED stop, never a silent
            // degradation into `Stuck`.
            //
            // A revocation the vendor announced outranks the exit code's generic
            // sentence ONLY when it is a hard stop: exit 52 is also Ralphy's own
            // malformed root, and blaming that for an enterprise Strict Mode is a
            // worse diagnosis than the one this arm exists to fix.
            //
            // The informational variants go LAST, below `actionable_stop`. On a
            // managed host the "MCP servers are disabled by administrator" notice
            // is in every log, so letting it pre-empt would report every exit 44
            // or 54 as a tool-server control — strictly worse than the sentence
            // the exit code already had.
            //
            // Gated on `!succeeded` throughout: a demotion on a run that still
            // went green must not cost it its `Done`.
            (!succeeded)
                .then(|| {
                    let rev = crate::revocation::detect_revocation(log);
                    rev.filter(|r| r.is_hard_stop())
                        .map(|r| r.message(exit_code, log))
                        .or_else(|| class.actionable_stop().map(str::to_string))
                        // More specific than the exit code's generic sentence,
                        // but never ahead of a hard-stop revocation (#255).
                        .or_else(|| {
                            crate::model::unknown_model_stop(log, model).map(|e| e.to_string())
                        })
                        .or_else(|| rev.map(|r| r.message(exit_code, log)))
                })
                .flatten()
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
mod tests;
