use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use tracing::info;

use crate::types::ResetAt;

/// A synthetic reset hint for a usage limit that carries **no** schedulable reset
/// time (e.g. Kimi's HTTP-403 account block, or any adapter's `Limit(None)`): treat
/// "unknown" as "retry in ~30 min". Returns `now + 25min` as RFC3339 —
/// [`wait_for_reset`](RunClock::wait_for_reset)'s 5-minute policy buffer then makes
/// the effective wake ~30 min out. Re-synthesised each cycle, so a still-limited
/// retry simply parks another window; the loop is unbounded until the run deadline
/// cuts it, the operator stops the run (`ralphy stop`, docs/adr/0054), or a human
/// interrupts (Ctrl-C) — which is the point: the operator, not a parseable reset,
/// decides when to give up (ADR-0030).
pub fn synthetic_reset() -> String {
    synthetic_instant(Local::now()).to_rfc3339()
}

/// The synthetic reset instant for a `now`: 25 minutes out.
fn synthetic_instant(now: DateTime<Local>) -> DateTime<Local> {
    now + chrono::Duration::minutes(25)
}

/// How a [`RunClock::wait_for_reset`] wait ended: the reset time arrived and the
/// run may resume, or the global deadline cut the wait short (deadline beats
/// resume).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    Resumed,
    DeadlinePassed,
}

/// The run's global deadline, behind a trait so "don't start a new issue past
/// the budget" is deterministically testable — an [`Instant`] can't be
/// fast-forwarded in a unit test, but a scripted clock can. The same indirection
/// lets [`wait_for_reset`](RunClock::wait_for_reset) return instantly under a
/// scripted clock instead of sleeping until a real reset time.
pub trait RunClock {
    fn deadline_passed(&self) -> bool;

    /// Block until the `reset` instant (plus a wait-policy buffer), polling so
    /// the wait stays interruptible and emits a heartbeat. Returns
    /// [`WaitOutcome::Resumed`] once the reset arrives, or
    /// [`WaitOutcome::DeadlinePassed`] if the global deadline is already past or
    /// passes during the wait (deadline beats resume).
    fn wait_for_reset(&self, reset: &ResetAt) -> WaitOutcome;
}

/// The production clock: a wall-clock deadline. `None` never expires.
pub struct WallClock {
    pub deadline: Option<Instant>,
}

impl RunClock for WallClock {
    fn deadline_passed(&self) -> bool {
        match self.deadline {
            Some(d) => Instant::now() >= d,
            None => false,
        }
    }

    fn wait_for_reset(&self, reset: &ResetAt) -> WaitOutcome {
        let target = wake_target(reset, Local::now());

        if self.deadline_passed() {
            return WaitOutcome::DeadlinePassed;
        }
        // No arbitrary wall ceiling on how far out a parsed reset may be: the agent's
        // output *is* the signal, and honouring "back at <ts>" — however distant — is
        // the Ralph-loop principle (act on what the output says; waiting days is a
        // human decision, ADR-0030). The wait is bounded only by the operator's own
        // levers: the run deadline (below), the progress-aware cap in `execute_phase`
        // (two no-commit resumes abandon the issue, ADR-0003 D1), the cooperative
        // stop checked at the top of the sleep loop in `sleep_until`, and Ctrl-C. A
        // garbage hint cannot park the run here because it never parses to a `target`
        // in the first place (it falls to the ~30-min synthetic cadence of
        // `wake_target`) — strict parsing, not a time cap, is the guard.
        if self.lands_beyond_deadline(target) {
            info!(reset = %reset.text, "reset lands beyond the run deadline — not waiting");
            return WaitOutcome::DeadlinePassed;
        }

        // Emit the display `reset` as the wake time-of-day (`HH:MM`, buffer included),
        // NOT the raw hint: the hint may be a full RFC3339 instant (Codex, or a
        // synthesised window) that reads badly in a card. The countdown downstream is
        // driven by `target_epoch`, so a bare `HH:MM` here is enough for the UI; the
        // raw hint stays as `hint` for the log.
        crate::emit::usage_limit_waiting(
            &target.format("%H:%M").to_string(),
            &reset.text,
            target.timestamp(),
        );
        self.sleep_until(target)
    }
}

impl WallClock {
    /// Whether waking at `target` would pass the global deadline. A reset beyond
    /// the global deadline never sleeps — the deadline wins the moment it would
    /// pass.
    fn lands_beyond_deadline(&self, target: DateTime<Local>) -> bool {
        let Some(d) = self.deadline else {
            return false;
        };
        let until_target = (target - Local::now()).num_milliseconds().max(0) as u64;
        Instant::now() + Duration::from_millis(until_target) >= d
    }

    /// Sleep in 1 s steps until `target`, with a heartbeat each minute. The
    /// operator's stop and the deadline end the sleep early.
    fn sleep_until(&self, target: DateTime<Local>) -> WaitOutcome {
        let mut last_heartbeat = Instant::now();
        loop {
            // The operator's stop (docs/adr/0054). This wait is unbounded by
            // design — a reset days out is honoured — so without this check it
            // is the ONE place a run could ignore a stop indefinitely.
            //
            // Answered as `DeadlinePassed` rather than a third `WaitOutcome`:
            // the caller's only question here is "resume or stop", both
            // implementors of the trait would need a new arm, and the runner's
            // `StopReason` override already reports the stop correctly.
            if crate::stop::requested() {
                info!("operator stop — abandoning the usage-limit wait");
                return WaitOutcome::DeadlinePassed;
            }
            if self.deadline_passed() {
                return WaitOutcome::DeadlinePassed;
            }
            if Local::now() >= target {
                crate::emit::reset_reached();
                return WaitOutcome::Resumed;
            }
            if last_heartbeat.elapsed() >= Duration::from_secs(60) {
                let remaining = (target - Local::now()).num_minutes().max(0);
                info!(remaining_min = remaining, "waiting for usage-limit reset");
                last_heartbeat = Instant::now();
            }
            thread::sleep(Duration::from_secs(1));
        }
    }
}

/// The wake time for a reset: its instant, or the synthetic reset when it has
/// none, plus the wait-policy buffer.
fn wake_target(reset: &ResetAt, now: DateTime<Local>) -> DateTime<Local> {
    // The 5-minute buffer is a wait policy (wake a little after the reset to
    // avoid re-limiting), applied here rather than baked into the parse.
    let buffer = chrono::Duration::minutes(5);
    // A reset the clock cannot read as an instant falls back to the synthetic
    // cadence — retry ~30 min out — so an unrecognised hint keeps trying until
    // the reset lands, the run deadline cuts it, or a human interrupts (#145).
    // The 25-min floor in `synthetic_instant` prevents a 0-second hot loop.
    let at = reset
        .at
        .map_or_else(|| synthetic_instant(now), |t| t.with_timezone(&Local));
    at + buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Build a fixed `DateTime<Local>` for deterministic `wake_target` tests.
    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap()
    }

    /// A reset that lands after the run deadline ends the wait before it
    /// starts: the deadline is 3 s out, so a wait that slept toward it would
    /// take at least 3 s.
    #[test]
    fn wait_for_reset_does_not_wait_for_a_reset_beyond_the_deadline() {
        let clock = WallClock {
            deadline: Some(Instant::now() + Duration::from_secs(3)),
        };
        let tomorrow = Local::now() + chrono::Duration::days(1);
        let started = Instant::now();
        let reset = ResetAt {
            text: tomorrow.to_rfc3339(),
            at: Some(tomorrow.fixed_offset()),
        };
        assert_eq!(clock.wait_for_reset(&reset), WaitOutcome::DeadlinePassed);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "took {:?}",
            started.elapsed()
        );
    }

    /// The wait-policy buffer every wake time carries.
    const BUFFER_MINUTES: i64 = 5;

    #[test]
    fn a_reset_without_an_instant_wakes_at_the_synthetic_reset() {
        let now = at(2026, 6, 9, 10, 0);
        let reset = ResetAt {
            text: "retry later".to_string(),
            at: None,
        };
        assert_eq!(
            wake_target(&reset, now),
            now + chrono::Duration::minutes(25 + BUFFER_MINUTES)
        );
    }

    #[test]
    fn a_reset_instant_wakes_at_that_instant_plus_the_buffer() {
        let now = at(2026, 6, 9, 10, 0);
        let instant = DateTime::parse_from_rfc3339("2026-06-09T18:00:00-03:00").unwrap();
        let reset = ResetAt {
            text: "6 PM".to_string(),
            at: Some(instant),
        };
        assert_eq!(
            wake_target(&reset, now).timestamp(),
            instant.timestamp() + BUFFER_MINUTES * 60
        );
    }
}
