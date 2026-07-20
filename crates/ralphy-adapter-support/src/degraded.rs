//! The vendor-neutral **API-degraded** clock for the headless runner.
//!
//! Mirrors the claude adapter's live-PTY `ApiWatch` (`api_watch.rs`) but over the
//! headless path: an adapter hands the runner a "this line is a degraded/retry
//! banner" predicate, and this clock turns a *persistent* degraded state
//! (continuous for ≥ [`API_DEGRADED_PING`]) into a matched pair of
//! [`API_DEGRADED_MSG`] / [`API_RECOVERED_MSG`] tracing events. The CLI decoder
//! maps those verbatim onto `RunEvent::ApiDegraded` / `ApiRecovered`, so a
//! degraded stretch looks identical to the operator whether it happened over a
//! PTY or over stdout/stderr (the same normalization docs/adr/0038 drew for the
//! idle reap).
//!
//! Detection is a loose per-vendor text heuristic: a miss degrades gracefully to
//! today's behaviour (the line counts as ordinary progress), never a wrong
//! terminal outcome. The vendor substrings live in each adapter's matcher; this
//! module holds only the neutral clock and the shared message constants.

use std::time::{Duration, Instant};

/// The degraded/recovered messages, owned by [`ralphy_core::emit`] (ADR-0039 §1)
/// and re-exported here so the historical `ralphy_adapter_support::…` paths keep
/// resolving. Emit them through [`ralphy_core::emit::api_degraded`] /
/// [`ralphy_core::emit::api_recovered`] — one helper per event is what keeps the
/// two execution paths from drifting into two different operator experiences.
pub use ralphy_core::emit::{API_DEGRADED_MSG, API_RECOVERED_MSG};

/// How long the degraded state must persist *continuously* before the event
/// fires — the criterion's ≥3-min gate. A shorter blip is a transient the child
/// recovers from on its own and stays silent.
const API_DEGRADED_PING: Duration = Duration::from_secs(180);

/// What the poll loop should do after advancing the degraded clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DegradedAction {
    /// Nothing to surface (healthy, a sub-ping blip, or already handled).
    None,
    /// The degraded state has persisted ≥ ping: emit [`API_DEGRADED_MSG`] once.
    Degraded,
    /// The stream recovered after a [`Degraded`](Self::Degraded) was emitted:
    /// emit the matching [`API_RECOVERED_MSG`].
    Recovered,
}

/// A single "time in degraded" clock. The clock starts when the degraded signal
/// is first observed, resets the moment a healthy sample arrives, and gates the
/// degraded event on continuous presence ≥ [`API_DEGRADED_PING`]. Uses an
/// injected `now` so the state machine unit-tests without real sleeping (mold of
/// `ApiWatch`).
pub struct DegradedWatch {
    ping: Duration,
    degraded_since: Option<Instant>,
    emitted: bool,
}

impl DegradedWatch {
    /// A fresh watch with the production ping ([`API_DEGRADED_PING`]).
    pub fn new() -> Self {
        Self {
            ping: API_DEGRADED_PING,
            degraded_since: None,
            emitted: false,
        }
    }

    /// Advance the clock. `degraded_now` is whether the most recent sample of the
    /// stream is in the degraded state. Returns the action the poll loop takes.
    pub fn poll(&mut self, now: Instant, degraded_now: bool) -> DegradedAction {
        if !degraded_now {
            // A healthy sample clears the clock so it strictly tracks *continuous*
            // degraded presence — a later degraded stretch cannot inherit an
            // earlier one's elapsed time. Recovery only pairs a degraded we
            // actually emitted (matched-pairs guard).
            self.degraded_since = None;
            if self.emitted {
                self.emitted = false;
                return DegradedAction::Recovered;
            }
            return DegradedAction::None;
        }
        let since = *self.degraded_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= self.ping && !self.emitted {
            self.emitted = true;
            DegradedAction::Degraded
        } else {
            DegradedAction::None
        }
    }
}

impl Default for DegradedWatch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enters_degraded_only_after_the_ping() {
        let mut w = DegradedWatch::new();
        let t0 = Instant::now();
        assert_eq!(w.poll(t0, true), DegradedAction::None);
        assert_eq!(
            w.poll(t0 + Duration::from_secs(179), true),
            DegradedAction::None
        );
        assert_eq!(
            w.poll(t0 + Duration::from_secs(180), true),
            DegradedAction::Degraded
        );
        // Fires exactly once — a later poll past the ping does not re-emit.
        assert_eq!(
            w.poll(t0 + Duration::from_secs(181), true),
            DegradedAction::None
        );
    }

    #[test]
    fn blip_under_ping_is_silent() {
        // A degraded stretch that clears before the ping never emits, so the
        // healthy poll returns None (not Recovered): no Recovered without a prior
        // emitted Degraded.
        let mut w = DegradedWatch::new();
        let t0 = Instant::now();
        assert_eq!(w.poll(t0, true), DegradedAction::None);
        assert_eq!(
            w.poll(t0 + Duration::from_secs(60), true),
            DegradedAction::None
        );
        assert_eq!(
            w.poll(t0 + Duration::from_secs(61), false),
            DegradedAction::None
        );
    }

    #[test]
    fn recovered_only_pairs_an_emitted_degraded() {
        let mut w = DegradedWatch::new();
        let t0 = Instant::now();
        assert_eq!(w.poll(t0, true), DegradedAction::None);
        assert_eq!(
            w.poll(t0 + Duration::from_secs(180), true),
            DegradedAction::Degraded
        );
        assert_eq!(
            w.poll(t0 + Duration::from_secs(200), false),
            DegradedAction::Recovered
        );
        // A second healthy poll does not re-emit Recovered.
        assert_eq!(
            w.poll(t0 + Duration::from_secs(201), false),
            DegradedAction::None
        );
    }

    #[test]
    fn healthy_sample_clears_the_clock() {
        // Degraded for 100s, a healthy sample, then degraded again: the ping fires
        // 180s AFTER the reappearance, not inheriting the earlier 100s.
        let mut w = DegradedWatch::new();
        let t0 = Instant::now();
        assert_eq!(w.poll(t0, true), DegradedAction::None); // clock starts at t0
        assert_eq!(
            w.poll(t0 + Duration::from_secs(100), true),
            DegradedAction::None
        );
        assert_eq!(
            w.poll(t0 + Duration::from_secs(101), false),
            DegradedAction::None // healthy → clock reset
        );
        // A fresh degraded stretch: Degraded fires 180s after the reappearance.
        assert_eq!(
            w.poll(t0 + Duration::from_secs(150), true),
            DegradedAction::None
        );
        assert_eq!(
            w.poll(t0 + Duration::from_secs(150 + 179), true),
            DegradedAction::None
        );
        assert_eq!(
            w.poll(t0 + Duration::from_secs(150 + 180), true),
            DegradedAction::Degraded
        );
    }
}
