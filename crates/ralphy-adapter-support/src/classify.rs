//! The shared, vendor-neutral **precedence** half of outcome classification
//! (ADR-0023). Each adapter still extracts its own [`CompletionSignals`] from
//! raw end state (Camada 1 — the ADR-0004 seam); this module owns only the fixed
//! ladder that orders those signals into a core [`Outcome`] (Camada 2), so the
//! precedence is verified once instead of drifting across three CLIs.

use ralphy_core::Outcome;

/// The vendor-extracted signals a session's raw end state reduces to, before the
/// shared precedence ladder orders them (ADR-0023 D1). Each field is filled by the
/// adapter — trustworthiness of `limit` and `exited_ok` are vendor decisions and
/// stay in extraction.
#[derive(Debug, Clone, Default)]
pub struct CompletionSignals {
    /// The `RALPHY_DONE_EXIT` completion sentinel was seen in the agent output.
    pub done: bool,
    /// A `RALPHY_BLOCKED_EXIT <reason>` sentinel was seen; carries the trimmed reason.
    pub blocked: Option<String>,
    /// A *trustworthy* usage limit fired. Outer `Some` = a limit was detected;
    /// inner `Option<String>` = the parsed reset hint (`None` when none was found).
    /// Trustworthiness is a vendor decision made during extraction (D1/D5).
    pub limit: Option<Option<String>>,
    /// A new commit landed this call. Progress signal only — it feeds streak
    /// heuristics, never the `Done` gate (ADR-0023 D3).
    pub committed: bool,
    /// The wall-clock timeout expired for this call.
    pub timed_out: bool,
    /// The exit was *trustworthy* (a successful/clean exit). Vendor-normalized:
    /// zero exit for Codex/OpenCode, `!timed_out` for Claude headless (D1).
    pub exited_ok: bool,
    /// The session errored in a way that invalidates a `Done` claim.
    pub errored: bool,
}

/// Apply the fixed ADR-0023 D2 precedence ladder to already-extracted signals.
/// A trustworthy `limit` outranks both `done` and `timeout` (resume-after-reset is
/// the conservative error); `done` requires a trustworthy exit and no error, and
/// never a commit (D3).
pub fn classify(s: CompletionSignals) -> Outcome {
    if let Some(reset) = s.limit {
        return Outcome::Limit(reset);
    }
    if s.done && s.exited_ok && !s.errored {
        return Outcome::Done;
    }
    if s.timed_out {
        return Outcome::Timeout;
    }
    if let Some(reason) = s.blocked {
        return Outcome::Blocked(reason);
    }
    Outcome::Stuck
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_matches_adr_0023_d2() {
        // (a) limit outranks timeout.
        assert_eq!(
            classify(CompletionSignals {
                limit: Some(Some("R".into())),
                timed_out: true,
                ..Default::default()
            }),
            Outcome::Limit(Some("R".into()))
        );
        // (b) limit outranks done.
        assert_eq!(
            classify(CompletionSignals {
                limit: Some(None),
                done: true,
                exited_ok: true,
                ..Default::default()
            }),
            Outcome::Limit(None)
        );
        // (c) Done needs no commit.
        assert_eq!(
            classify(CompletionSignals {
                done: true,
                committed: false,
                exited_ok: true,
                errored: false,
                ..Default::default()
            }),
            Outcome::Done
        );
        // (d) Done needs a clean exit: no exited_ok → falls through to timeout…
        assert_eq!(
            classify(CompletionSignals {
                done: true,
                exited_ok: false,
                timed_out: true,
                ..Default::default()
            }),
            Outcome::Timeout
        );
        // …and to Stuck when nothing else fires.
        assert_eq!(
            classify(CompletionSignals {
                done: true,
                exited_ok: false,
                ..Default::default()
            }),
            Outcome::Stuck
        );
        // (e) timeout outranks blocked.
        assert_eq!(
            classify(CompletionSignals {
                timed_out: true,
                blocked: Some("x".into()),
                ..Default::default()
            }),
            Outcome::Timeout
        );
        // (f) blocked outranks stuck.
        assert_eq!(
            classify(CompletionSignals {
                blocked: Some("x".into()),
                ..Default::default()
            }),
            Outcome::Blocked("x".into())
        );
        // (g) all-false → Stuck.
        assert_eq!(classify(CompletionSignals::default()), Outcome::Stuck);
    }
}
