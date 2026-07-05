use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Local, NaiveTime, Weekday};
use tracing::info;

/// Upper bound on a single usage-limit wait. A reset hint that resolves farther
/// out than this is treated as a stop (`DeadlinePassed`) rather than parked on —
/// guards against a malformed/hostile hint parking the run for the unbounded
/// issue horizon (~365 days) when no run-level deadline is set.
const MAX_RESET_WAIT: Duration = Duration::from_secs(12 * 60 * 60);

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

    /// Block until the parsed `reset` time (plus a wait-policy buffer), polling so
    /// the wait stays interruptible and emits a heartbeat. Returns
    /// [`WaitOutcome::Resumed`] once the reset arrives, or
    /// [`WaitOutcome::DeadlinePassed`] if the global deadline is already past or
    /// passes during the wait (deadline beats resume).
    fn wait_for_reset(&self, reset: &str) -> WaitOutcome;
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

    fn wait_for_reset(&self, reset: &str) -> WaitOutcome {
        // The 5-minute buffer is a wait policy (wake a little after the reset to
        // avoid re-limiting), applied here rather than baked into `next_reset`.
        let buffer = chrono::Duration::minutes(5);
        let target = match next_reset(reset, Local::now()) {
            Some(t) => t + buffer,
            // An unparseable reset should never reach here (the loop only calls
            // wait_for_reset when a reset was parsed); resume immediately rather
            // than sleep on a guess.
            None => return WaitOutcome::Resumed,
        };

        if self.deadline_passed() {
            return WaitOutcome::DeadlinePassed;
        }
        // A reset farther out than the max wait is a stop, regardless of whether a
        // run deadline is set — without this, a hint resolving to the unbounded
        // issue horizon would park the run for ~365 days.
        if target - Local::now()
            > chrono::Duration::from_std(MAX_RESET_WAIT)
                .unwrap_or_else(|_| chrono::Duration::hours(12))
        {
            info!(%reset, "reset lands beyond the max wait — not waiting");
            return WaitOutcome::DeadlinePassed;
        }
        // A reset beyond the global deadline never sleeps — the deadline wins the
        // moment it would pass.
        if let Some(d) = self.deadline {
            let until_target = (target - Local::now()).num_milliseconds().max(0) as u64;
            if Instant::now() + Duration::from_millis(until_target) >= d {
                info!(%reset, "reset lands beyond the run deadline — not waiting");
                return WaitOutcome::DeadlinePassed;
            }
        }

        info!(%reset, target = %target.format("%Y-%m-%d %H:%M"), target_epoch = target.timestamp(), "usage limit — waiting for reset");
        let mut last_heartbeat = Instant::now();
        loop {
            if self.deadline_passed() {
                return WaitOutcome::DeadlinePassed;
            }
            if Local::now() >= target {
                info!("reset reached — resuming");
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

/// The next clock occurrence of a parsed reset hint relative to `now`. `reset` is
/// one of: an absolute RFC3339 instant (`"2026-06-09T18:00:00Z"`, as some adapters
/// emit), a bare `"HH:mm"`, or a weekday-qualified `"Wkd HH:mm"` (the relative
/// forms others emit). An absolute instant is unambiguous and used as-is — `now` is ignored. A
/// bare time resolves to today, rolled to tomorrow when already past `now`; a
/// weekday-qualified time resolves to the next date carrying that weekday (today
/// only when the time is still ahead, else next week). Pure over its inputs so the
/// rollover edge cases unit-test without sleeping. Returns `None` on an
/// unparseable hint.
fn next_reset(reset: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    // Strip trailing sentence punctuation an adapter may leave on the hint (e.g.
    // "… Try again at 2026-06-09T18:00:00Z.").
    let trimmed = reset.trim().trim_end_matches('.').trim();

    // An absolute RFC3339 instant is unambiguous (carries its own date and zone):
    // use it directly, converted to local time. No next-occurrence guess is needed,
    // unlike the relative forms handled below. Try the whole hint, then its leading
    // token — the datetime may be trailed by prose ("…Z (in 3 hours)"). The relative
    // forms never parse as RFC3339 ("Fri"/"15:00" both fail), so this stays additive.
    let leading = trimmed.split_whitespace().next().unwrap_or(trimmed);
    for cand in [trimmed, leading.trim_end_matches('.')] {
        if let Ok(dt) = DateTime::parse_from_rfc3339(cand) {
            return Some(dt.with_timezone(&Local));
        }
    }

    let (weekday, hhmm) = match trimmed.split_once(char::is_whitespace) {
        Some((wd, rest)) => (Some(parse_weekday(wd.trim())?), rest.trim()),
        // Use `trimmed` (trailing punctuation already stripped), not the raw
        // `reset`, so a bare hint like "15:00." parses instead of failing.
        None => (None, trimmed),
    };
    let (h, m) = hhmm.split_once(':')?;
    let hour: u32 = h.parse().ok()?;
    let min: u32 = m.parse().ok()?;
    let time = NaiveTime::from_hms_opt(hour, min, 0)?;

    let today = now.date_naive();
    let target_date = match weekday {
        None => {
            if now.time() < time {
                today
            } else {
                today + chrono::Duration::days(1)
            }
        }
        Some(wd) => {
            let cur = today.weekday().num_days_from_monday() as i64;
            let tgt = wd.num_days_from_monday() as i64;
            let mut days = (tgt - cur).rem_euclid(7);
            // Same weekday today: keep today only if the time is still ahead.
            if days == 0 && now.time() >= time {
                days = 7;
            }
            today + chrono::Duration::days(days)
        }
    };
    target_date
        .and_time(time)
        .and_local_timezone(Local)
        .single()
}

/// Parse a three-letter weekday abbreviation (case-insensitive) into a chrono
/// [`Weekday`]. Returns `None` for anything else.
fn parse_weekday(s: &str) -> Option<Weekday> {
    match s.to_lowercase().as_str() {
        "mon" => Some(Weekday::Mon),
        "tue" => Some(Weekday::Tue),
        "wed" => Some(Weekday::Wed),
        "thu" => Some(Weekday::Thu),
        "fri" => Some(Weekday::Fri),
        "sat" => Some(Weekday::Sat),
        "sun" => Some(Weekday::Sun),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Build a fixed `DateTime<Local>` for deterministic `next_reset` tests.
    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap()
    }

    #[test]
    fn next_reset_same_day_past_rolls_to_tomorrow() {
        // 2026-06-09 is a Tuesday. Now 16:00, bare reset 15:00 already past today.
        let now = at(2026, 6, 9, 16, 0);
        let got = next_reset("15:00", now).unwrap();
        assert_eq!(
            got,
            at(2026, 6, 10, 15, 0),
            "bare past time rolls to tomorrow"
        );
    }

    #[test]
    fn next_reset_future_same_day_stays_today() {
        let now = at(2026, 6, 9, 10, 0);
        let got = next_reset("15:00", now).unwrap();
        assert_eq!(got, at(2026, 6, 9, 15, 0), "bare future time stays today");
    }

    #[test]
    fn next_reset_bare_time_tolerates_trailing_period() {
        // A bare hint with trailing sentence punctuation must parse like the
        // absolute form does, not fall through to None.
        let now = at(2026, 6, 9, 10, 0);
        let got = next_reset("15:00.", now).unwrap();
        assert_eq!(
            got,
            at(2026, 6, 9, 15, 0),
            "bare time strips trailing period"
        );
    }

    #[test]
    fn next_reset_weekday_picks_next_matching_date() {
        // Now is Tuesday 2026-06-09; the next Friday is 2026-06-12.
        let now = at(2026, 6, 9, 10, 0);
        let got = next_reset("Fri 09:00", now).unwrap();
        assert_eq!(
            got,
            at(2026, 6, 12, 9, 0),
            "weekday picks the next matching date"
        );
    }

    #[test]
    fn next_reset_same_weekday_past_rolls_a_week() {
        // Today is Tuesday; a Tuesday reset already past today lands next Tuesday.
        let now = at(2026, 6, 9, 16, 0);
        let got = next_reset("Tue 15:00", now).unwrap();
        assert_eq!(
            got,
            at(2026, 6, 16, 15, 0),
            "same weekday past rolls a week"
        );
    }

    #[test]
    fn next_reset_unparseable_is_none() {
        let now = at(2026, 6, 9, 10, 0);
        assert_eq!(next_reset("not a time", now), None);
    }

    #[test]
    fn next_reset_absolute_rfc3339_used_directly() {
        // An absolute instant ignores `now` and resolves to the
        // exact instant it names. Compare epochs so the assertion is timezone-
        // independent (the result is the same instant regardless of local zone).
        let now = at(2026, 6, 9, 10, 0);
        let expected = DateTime::parse_from_rfc3339("2026-06-09T18:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(
            next_reset("2026-06-09T18:00:00Z", now).unwrap().timestamp(),
            expected
        );
    }

    #[test]
    fn next_reset_absolute_tolerates_trailing_period() {
        // Some adapters emit a sentence: "… Try again at 2026-06-09T18:00:00Z."
        let now = at(2026, 6, 9, 10, 0);
        let expected = DateTime::parse_from_rfc3339("2026-06-09T18:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(
            next_reset("2026-06-09T18:00:00Z.", now)
                .unwrap()
                .timestamp(),
            expected
        );
    }

    #[test]
    fn next_reset_absolute_ignores_trailing_prose() {
        // The datetime may be trailed with prose: "…Z (in 3 hours)".
        let now = at(2026, 6, 9, 10, 0);
        let expected = DateTime::parse_from_rfc3339("2026-06-09T18:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(
            next_reset("2026-06-09T18:00:00Z (in 3 hours)", now)
                .unwrap()
                .timestamp(),
            expected
        );
    }

    #[test]
    fn next_reset_absolute_honours_offset() {
        let now = at(2026, 6, 9, 10, 0);
        let expected = DateTime::parse_from_rfc3339("2026-06-09T15:00:00-03:00")
            .unwrap()
            .timestamp();
        assert_eq!(
            next_reset("2026-06-09T15:00:00-03:00", now)
                .unwrap()
                .timestamp(),
            expected
        );
    }
}
