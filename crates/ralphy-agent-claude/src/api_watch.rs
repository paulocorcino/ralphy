//! Degraded-state watch over the live PTY stream for the child's API failures.
//!
//! During `execute()` the Claude Code child can hit transient API failures and
//! render a banner (`Waiting for API response…` / `API Error: Server error
//! mid-response`) while it retries internally. This watch treats a persistent
//! banner as a single "time in degraded" clock and drives three behaviours off
//! it: an immediate local retry indicator, a matched pair of
//! `ApiDegraded`/`ApiRecovered` events once the banner persists ≥ `ping`, and a
//! one-shot child re-spawn once it persists past `kill`.
//!
//! All timing knowledge lives here so the CLI sinks stay dumb — the events only
//! fire after the adapter's own gate (see the module docs in `interactive.rs`
//! for the sibling `LoginTuiWatch`). Detection is a loose text heuristic: a miss
//! is graceful degradation to today's wall-timeout, never a wrong terminal
//! outcome.

use std::time::{Duration, Instant};

/// What the PTY loop should do after a `poll` of the degraded clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiWatchAction {
    /// Nothing to surface (healthy, a sub-`ping` blip, or already handled).
    None,
    /// The banner has persisted ≥ `ping`: emit `RunEvent::ApiDegraded` once.
    Degraded,
    /// Transcript activity resumed after a `Degraded` was emitted: emit the
    /// matching `RunEvent::ApiRecovered`.
    Recovered,
    /// The banner has persisted past `kill`: re-spawn the child once.
    Respawn,
}

/// Loose, case-insensitive banner matcher over flattened PTY bytes. Keys on the
/// two distinctive substrings from the child's API-failure banner (issue #149).
/// A miss falls through to today's wall-timeout, so a false negative is safe.
pub(crate) fn is_api_degraded_output(raw: &[u8]) -> bool {
    let text = crate::interactive::strip_pty_escapes(raw).to_lowercase();
    text.contains("waiting for api response") || text.contains("server error mid-response")
}

/// Rolling watch that keeps a single "time in degraded" clock over the live PTY
/// stream. The clock starts when the banner is first seen, resets when
/// transcript activity resumes, and gates the degraded event (≥ `ping`) and the
/// one-shot re-spawn (≥ `kill`).
pub(crate) struct ApiWatch {
    buf: Vec<u8>,
    degraded_since: Option<Instant>,
    degraded_emitted: bool,
    ping: Duration,
    kill: Duration,
}

impl ApiWatch {
    /// Plenty for the banner tail; the TUI redraws, so the signature recurs.
    const MAX_BUF: usize = 32 * 1024;

    fn with_thresholds(ping: Duration, kill: Duration) -> Self {
        Self {
            buf: Vec::new(),
            degraded_since: None,
            degraded_emitted: false,
            ping,
            kill,
        }
    }

    /// `ping = 3 min` (the criterion's ≥3-min degraded gate), `kill = 17 min`
    /// (mid-range of the body's "~15–20 min" short kill).
    pub(crate) fn new() -> Self {
        Self::with_thresholds(Duration::from_secs(180), Duration::from_secs(17 * 60))
    }

    /// Accumulate a PTY chunk (keeps the most recent [`Self::MAX_BUF`] bytes).
    pub(crate) fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > Self::MAX_BUF {
            let cut = self.buf.len() - Self::MAX_BUF;
            self.buf.drain(..cut);
        }
    }

    fn banner_present(&self) -> bool {
        is_api_degraded_output(&self.buf)
    }

    /// Advance the degraded clock. `transcript_advanced` is `true` when the
    /// child's JSONL transcript grew since the last poll — proof the model loop
    /// resumed, which clears any degraded state. Returns the action the PTY loop
    /// should take. Uses an injected `now` so the state machine unit-tests
    /// without real sleeping.
    pub(crate) fn poll(&mut self, now: Instant, transcript_advanced: bool) -> ApiWatchAction {
        if transcript_advanced {
            // Activity resumed: recovery only pairs a degraded we actually emitted.
            let was = self.degraded_emitted;
            self.degraded_since = None;
            self.degraded_emitted = false;
            self.buf.clear();
            return if was {
                ApiWatchAction::Recovered
            } else {
                ApiWatchAction::None
            };
        }
        if !self.banner_present() {
            return ApiWatchAction::None;
        }
        let since = *self.degraded_since.get_or_insert(now);
        let elapsed = now.saturating_duration_since(since);
        if elapsed >= self.kill {
            ApiWatchAction::Respawn
        } else if elapsed >= self.ping && !self.degraded_emitted {
            self.degraded_emitted = true;
            ApiWatchAction::Degraded
        } else {
            ApiWatchAction::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic banner chunk carrying one of the two distinctive substrings,
    /// wrapped in cursor-forward escapes like the real TUI separates words.
    const BANNER: &[u8] =
        b"\x1b[38;2;153;153;153mWaiting\x1b[1Cfor\x1b[1CAPI\x1b[1Cresponse\x1b[m\r\n";

    #[test]
    fn enters_degraded_pings_after_3min() {
        let mut w = ApiWatch::new();
        w.feed(BANNER);
        let t0 = Instant::now();
        assert_eq!(w.poll(t0, false), ApiWatchAction::None);
        assert_eq!(
            w.poll(t0 + Duration::from_secs(120), false),
            ApiWatchAction::None
        );
        assert_eq!(
            w.poll(t0 + Duration::from_secs(180), false),
            ApiWatchAction::Degraded
        );
        // Fires exactly once — a second poll past ping does not re-ping.
        assert_eq!(
            w.poll(t0 + Duration::from_secs(181), false),
            ApiWatchAction::None
        );
        // Transcript activity resumes → the matched recovery.
        assert_eq!(
            w.poll(t0 + Duration::from_secs(200), true),
            ApiWatchAction::Recovered
        );
    }

    #[test]
    fn blip_under_3min_is_silent() {
        // A banner that clears before the ping never emits, so its recovery is
        // silent too (matched-pairs guard: no Recovered without a prior Degraded).
        let mut w = ApiWatch::new();
        w.feed(BANNER);
        let t0 = Instant::now();
        assert_eq!(w.poll(t0, false), ApiWatchAction::None);
        assert_eq!(
            w.poll(t0 + Duration::from_secs(60), true),
            ApiWatchAction::None
        );
    }

    #[test]
    fn respawn_after_kill() {
        let mut w = ApiWatch::new();
        w.feed(BANNER);
        let t0 = Instant::now();
        assert_eq!(w.poll(t0, false), ApiWatchAction::None);
        assert_eq!(
            w.poll(t0 + Duration::from_secs(180), false),
            ApiWatchAction::Degraded
        );
        assert_eq!(
            w.poll(t0 + Duration::from_secs(17 * 60), false),
            ApiWatchAction::Respawn
        );
    }

    #[test]
    fn graceful_miss_no_banner_stays_none() {
        // ANSI-heavy healthy output with no banner: even far past the kill
        // duration the watch never escalates — the loop falls through to today's
        // wall-timeout instead of a wrong terminal outcome.
        let healthy = b"\x1b[38;2;153;153;153m\x1b[17;3H?\x1b[1Cfor\x1b[1Cshortcuts\
            \x1b[18;83H\x1b[1Chigh\x1b[1C\xc2\xb7\x1b[1C/effort\x1b[m\r\n\
            Running\x1b[1Ccargo\x1b[1Ctest...\r\n";
        let mut w = ApiWatch::new();
        w.feed(healthy);
        let t0 = Instant::now();
        assert_eq!(
            w.poll(t0 + Duration::from_secs(20 * 60), false),
            ApiWatchAction::None
        );
    }

    #[test]
    fn is_api_degraded_output_matches_banners_only() {
        // Both distinctive banner shapes match, over the cursor-move separators.
        assert!(is_api_degraded_output(
            b"\x1b[2mWaiting\x1b[1Cfor\x1b[1CAPI\x1b[1Cresponse\x1b[m"
        ));
        assert!(is_api_degraded_output(
            b"API\x1b[1CError:\x1b[1CServer\x1b[1Cerror\x1b[1Cmid-response"
        ));
        // A healthy status line does not match, nor does prose merely naming "api".
        assert!(!is_api_degraded_output(
            b"\x1b[2m?\x1b[1Cfor\x1b[1Cshortcuts\x1b[m"
        ));
        assert!(!is_api_degraded_output(
            b"the api call returned 200 \xc2\xb7 continuing"
        ));
    }
}
