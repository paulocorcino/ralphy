//! The global login throttle (docs/adr/0032 amendment §D).

use std::time::{Duration, Instant};

/// A simple global login throttle (amendment §D): a single-operator daemon does
/// not need per-IP buckets, only a brake on online brute force of the 6-digit
/// TOTP. After [`LOCKOUT_THRESHOLD`] consecutive failures it locks out for a
/// window that doubles each further failure, capped at [`LOCKOUT_MAX_SECS`].
pub(super) struct LoginThrottle {
    failures: u32,
    locked_until: Option<Instant>,
}

/// Consecutive failures tolerated before the lockout engages.
pub(super) const LOCKOUT_THRESHOLD: u32 = 5;

/// The base lockout window (doubles per failure past the threshold).
pub(super) const LOCKOUT_BASE_SECS: u64 = 5;

/// The lockout ceiling — never brick the operator out permanently.
pub(super) const LOCKOUT_MAX_SECS: u64 = 300;

impl LoginThrottle {
    pub(super) fn new() -> LoginThrottle {
        LoginThrottle {
            failures: 0,
            locked_until: None,
        }
    }

    /// `Err(secs)` with the remaining lockout, or `Ok(())` when a try may proceed.
    pub(super) fn check(&self, now: Instant) -> std::result::Result<(), u64> {
        match self.locked_until {
            Some(until) if until > now => Err((until - now).as_secs().max(1)),
            _ => Ok(()),
        }
    }

    pub(super) fn record_failure(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= LOCKOUT_THRESHOLD {
            let over = self.failures - LOCKOUT_THRESHOLD;
            let secs = LOCKOUT_BASE_SECS
                .saturating_mul(1u64 << over.min(6))
                .min(LOCKOUT_MAX_SECS);
            self.locked_until = Some(now + Duration::from_secs(secs));
        }
    }

    pub(super) fn reset(&mut self) {
        self.failures = 0;
        self.locked_until = None;
    }
}

#[cfg(test)]
mod tests;
