//! Idle watchdog: the vendor-neutral "the child stopped making progress" clock.
//!
//! This is the liveness backstop the per-issue wall-clock cap used to improvise
//! (docs/adr/0038). The distinction it exists to draw:
//!
//! - the **per-issue cap** answers *how long has this issue run* — a productivity
//!   budget, opt-in, and blind to whether the child is healthy;
//! - the **idle watchdog** answers *how long since anything happened* — a liveness
//!   signal, on by default, and blind to how long the issue has legitimately run.
//!
//! Only the second one can tell a wedged child from a slow one, which is why a
//! finite per-issue default was the wrong instrument for issue #150's symptom
//! ("child stuck on `Waiting for API response` and never leaving"). It is also
//! the only thing that catches a provider quota block the child retries
//! *silently*: no stderr matcher can see a failure that is never printed, but the
//! resulting silence is unmistakable.
//!
//! What counts as "progress" is the caller's choice and it matters more than the
//! threshold — see [`ProgressBeat`] for the headless signal and the interactive
//! PTY loop for why bytes are not usable as one there.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The canonical log message every path emits when the watchdog reaps a child.
///
/// The CLI turns run events into `RunEvent`s by matching the tracing message
/// verbatim, so this being one shared constant is what makes an idle reap look
/// **identical** to the operator whether it happened over a PTY or over stdout:
/// same event, same Telegram ping, same CloudEvent. Emit it at `INFO` — the
/// decoder short-circuits `WARN`/`ERROR` into a generic notice, which would sink
/// this back into per-path noise.
pub const IDLE_REAPED_MSG: &str = "idle watchdog — no progress, reaping the child";

/// A shared "something happened at T" beacon, written by whichever thread
/// observes progress and read by the poll loop.
///
/// Stored as milliseconds since a fixed `base` [`Instant`] because [`Instant`]
/// itself is not atomic; the base makes the stored value monotonic and small.
/// Cloning the [`Arc`] is how a reader thread and the poll loop share one clock.
#[derive(Debug)]
pub struct ProgressBeat {
    base: Instant,
    since_base_ms: AtomicU64,
}

impl ProgressBeat {
    /// A beacon that starts "fresh" at `base` — spawning counts as progress, so a
    /// child that takes a while to say anything at all is not instantly idle.
    pub fn new(base: Instant) -> Arc<Self> {
        Arc::new(Self {
            base,
            since_base_ms: AtomicU64::new(0),
        })
    }

    /// Record progress observed at `now`. Cheap enough to call per output line.
    ///
    /// `Ordering::Relaxed` is deliberate: the only consumer is a 500ms poll loop
    /// comparing elapsed time, so a tick of staleness is irrelevant and there is
    /// no other memory being published alongside it.
    pub fn beat(&self, now: Instant) {
        let ms = now.saturating_duration_since(self.base).as_millis() as u64;
        self.since_base_ms.store(ms, Ordering::Relaxed);
    }

    /// How long since the last recorded progress, as of `now`.
    pub fn idle_for(&self, now: Instant) -> Duration {
        let last = self.base + Duration::from_millis(self.since_base_ms.load(Ordering::Relaxed));
        now.saturating_duration_since(last)
    }
}

/// The idle threshold itself: a window, or `None` when the watchdog is off.
///
/// Kept as a tiny type rather than a bare `Option<Duration>` so the `0 = disabled`
/// convention is decided in exactly one place instead of at every call site, and
/// so [`expired`](Self::expired) reads as an intent at the poll loop.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdleWatch {
    window: Option<Duration>,
}

impl IdleWatch {
    /// Build from a minutes knob. `0` disables the watchdog entirely — the
    /// operator keeps the ability to run with no liveness net at all.
    pub fn from_minutes(minutes: u64) -> Self {
        Self {
            window: (minutes > 0).then(|| Duration::from_secs(minutes.saturating_mul(60))),
        }
    }

    /// Build from an exact window. The operator-facing knob is whole minutes;
    /// this exists so tests can exercise the real kill path in seconds instead of
    /// waiting out a production-sized window.
    pub fn from_window(window: Duration) -> Self {
        Self {
            window: (!window.is_zero()).then_some(window),
        }
    }

    /// The configured window, `None` when disabled.
    pub fn window(&self) -> Option<Duration> {
        self.window
    }

    /// Whether the silence measured by `beat` has outlived the window. Always
    /// `false` when disabled.
    pub fn expired(&self, beat: &ProgressBeat, now: Instant) -> bool {
        self.window
            .is_some_and(|w| beat.idle_for(now) >= w && !w.is_zero())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_minutes_disables_the_watchdog() {
        let w = IdleWatch::from_minutes(0);
        assert!(w.window().is_none());
        let t0 = Instant::now();
        let beat = ProgressBeat::new(t0);
        // Even an absurd silence never expires a disabled watchdog.
        assert!(!w.expired(&beat, t0 + Duration::from_secs(10 * 3600)));
    }

    #[test]
    fn silence_below_the_window_does_not_expire() {
        let w = IdleWatch::from_minutes(20);
        let t0 = Instant::now();
        let beat = ProgressBeat::new(t0);
        assert!(!w.expired(&beat, t0 + Duration::from_secs(19 * 60)));
    }

    #[test]
    fn silence_past_the_window_expires() {
        let w = IdleWatch::from_minutes(20);
        let t0 = Instant::now();
        let beat = ProgressBeat::new(t0);
        assert!(w.expired(&beat, t0 + Duration::from_secs(20 * 60)));
        assert!(w.expired(&beat, t0 + Duration::from_secs(21 * 60)));
    }

    #[test]
    fn progress_rearms_the_clock() {
        // The property that separates this from a wall-clock cap: total elapsed
        // time is irrelevant, only the gap since the last sign of life counts.
        let w = IdleWatch::from_minutes(20);
        let t0 = Instant::now();
        let beat = ProgressBeat::new(t0);

        beat.beat(t0 + Duration::from_secs(19 * 60));
        assert!(!w.expired(&beat, t0 + Duration::from_secs(38 * 60)));

        beat.beat(t0 + Duration::from_secs(38 * 60));
        assert!(!w.expired(&beat, t0 + Duration::from_secs(57 * 60)));

        // A child that has run for an hour but went quiet 20 min ago is idle.
        assert!(w.expired(&beat, t0 + Duration::from_secs(58 * 60)));
    }

    #[test]
    fn a_fresh_beacon_starts_from_its_base() {
        // Spawning counts as progress: a child slow to emit its first line is not
        // instantly declared idle.
        let t0 = Instant::now();
        let beat = ProgressBeat::new(t0);
        assert_eq!(beat.idle_for(t0), Duration::ZERO);
        assert_eq!(
            beat.idle_for(t0 + Duration::from_secs(90)),
            Duration::from_secs(90)
        );
    }
}
