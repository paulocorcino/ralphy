use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Local, NaiveDateTime, NaiveTime, Weekday};
use tracing::info;

/// Upper bound on a single usage-limit wait. A reset hint that resolves farther
/// out than this is treated as a stop (`DeadlinePassed`) rather than parked on —
/// guards against a malformed/hostile hint parking the run for the unbounded
/// issue horizon (~365 days) when no run-level deadline is set.
const MAX_RESET_WAIT: Duration = Duration::from_secs(12 * 60 * 60);

/// A synthetic reset hint for a usage limit that carries **no** schedulable reset
/// time (e.g. Kimi's HTTP-403 account block, or any adapter's `Limit(None)`): treat
/// "unknown" as "retry in ~30 min". Returns `now + 25min` as RFC3339 —
/// [`wait_for_reset`](RunClock::wait_for_reset)'s 5-minute policy buffer then makes
/// the effective wake ~30 min out. Re-synthesised each cycle, so a still-limited
/// retry simply parks another window; the loop is unbounded until the run deadline
/// cuts it or a human interrupts (Ctrl-C), which is the point — the operator, not a
/// parseable reset, decides when to give up (ADR-0030).
pub fn synthetic_reset() -> String {
    (Local::now() + chrono::Duration::minutes(25)).to_rfc3339()
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
            // The adapter parsed *some* reset string, but this clock cannot turn
            // it into a wake time (an unrecognised format the adapter accepts but
            // `next_reset` does not). Resuming immediately here hot-loops the
            // execute retry until the progress-aware cap abandons the issue with a
            // 0-second "wait" (issue #145). Fail safe instead: stop-and-report on
            // the limit, exactly as a `Limit(None)` would, so an operator re-runs
            // rather than the run silently burning attempts.
            None => {
                info!(%reset, "usage-limit reset hint not understood by the clock — stopping instead of waiting");
                return WaitOutcome::DeadlinePassed;
            }
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

        // Emit the display `reset` as the wake time-of-day (`HH:MM`, buffer included),
        // NOT the raw hint: the hint may be a full RFC3339 instant (Codex, or a
        // synthesised window) that reads badly in a card. The countdown downstream is
        // driven by `target_epoch`, so a bare `HH:MM` here is enough for the UI; the
        // raw hint stays as `hint` for the log.
        info!(
            reset = %target.format("%H:%M"),
            hint = %reset,
            target_epoch = target.timestamp(),
            "usage limit — waiting for reset"
        );
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
/// emit), an absolute month-name date-time (`"Jun 10th, 2026 12:23 AM"`, as Codex
/// emits), a bare 24-hour `"HH:mm"`, a bare 12-hour `"H:MM AM/PM"` (`"6:13 PM"`,
/// also Codex), or a weekday-qualified `"Wkd HH:mm"` (the relative forms others
/// emit). An absolute instant is unambiguous and used as-is — `now` is ignored. A
/// bare time resolves to today, rolled to tomorrow when already past `now`; a
/// weekday-qualified time resolves to the next date carrying that weekday (today
/// only when the time is still ahead, else next week). Pure over its inputs so the
/// rollover edge cases unit-test without sleeping. Returns `None` on an
/// unparseable hint.
///
/// The Codex adapter accepts human-formatted hints verbatim (whatever follows
/// "try again at "), so this clock must understand the same forms it emits — a
/// parser this side that is stricter than the adapter's makes the run *think* it
/// has a wake time and then refuse to wait on it (issue #145).
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

    // An absolute month-name date-time carries its own date, like RFC3339 but in the
    // human form Codex emits ("Jun 10th, 2026 12:23 AM"). No local zone is stated, so
    // it is read as local wall-clock time.
    if let Some(dt) = parse_month_name_datetime(trimmed) {
        return Some(dt);
    }

    // A bare 12-hour time ("6:13 PM") is relative: resolve to its next occurrence
    // like a bare 24-hour time. Checked before the weekday/`HH:mm` split below,
    // whose `parse_weekday("6:13")` would otherwise reject it.
    if let Some(time) = parse_12h_time(trimmed) {
        return next_occurrence(time, None, now);
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

    next_occurrence(time, weekday, now)
}

/// The next date/time carrying `time`, relative to `now`: today when still ahead
/// (else tomorrow) for a bare time, or the next date on `weekday` when qualified
/// (today only when the time is still ahead, else next week).
fn next_occurrence(
    time: NaiveTime,
    weekday: Option<Weekday>,
    now: DateTime<Local>,
) -> Option<DateTime<Local>> {
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

/// Parse a bare 12-hour clock time like `"6:13 PM"` (Codex's format). Returns
/// `None` when the AM/PM marker is absent, so 24-hour `"18:13"` and
/// weekday-qualified hints fall through to their own parsers.
fn parse_12h_time(s: &str) -> Option<NaiveTime> {
    let upper = s.trim().to_uppercase();
    if !upper.ends_with("AM") && !upper.ends_with("PM") {
        return None;
    }
    // chrono's numeric specifiers accept an unpadded hour, so "6:13 PM" parses.
    NaiveTime::parse_from_str(&upper, "%I:%M %p").ok()
}

/// Parse an absolute month-name date-time like `"Jun 10th, 2026 12:23 AM"`
/// (Codex's format) into a local instant. Ordinal suffixes and commas are
/// stripped first so chrono's format specifiers match. Returns `None` when the
/// hint is not this form.
fn parse_month_name_datetime(s: &str) -> Option<DateTime<Local>> {
    let cleaned = s.replace(',', " ");
    let normalized = cleaned
        .split_whitespace()
        .map(strip_ordinal_suffix)
        .collect::<Vec<_>>()
        .join(" ");
    for fmt in [
        "%b %d %Y %I:%M %p",
        "%B %d %Y %I:%M %p",
        "%b %d %Y %H:%M",
        "%B %d %Y %H:%M",
    ] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(&normalized, fmt) {
            return ndt.and_local_timezone(Local).single();
        }
    }
    None
}

/// Drop an ordinal suffix (`st`/`nd`/`rd`/`th`) from a token that is otherwise all
/// digits, so `"10th"` becomes `"10"`. Any other token is returned unchanged.
fn strip_ordinal_suffix(tok: &str) -> &str {
    for suf in ["st", "nd", "rd", "th"] {
        if let Some(stem) = tok.strip_suffix(suf) {
            if !stem.is_empty() && stem.chars().all(|c| c.is_ascii_digit()) {
                return stem;
            }
        }
    }
    tok
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
    fn next_reset_12h_pm_future_same_day_stays_today() {
        // Codex emits a bare 12-hour time ("try again at 6:13 PM"). Before the
        // fix for #145 this parsed to None and the run refused to wait.
        let now = at(2026, 6, 9, 15, 20);
        let got = next_reset("6:13 PM", now).unwrap();
        assert_eq!(
            got,
            at(2026, 6, 9, 18, 13),
            "6:13 PM resolves to 18:13 today"
        );
    }

    #[test]
    fn next_reset_12h_pm_past_rolls_to_tomorrow() {
        // Same 12-hour form, but already past today, rolls to tomorrow.
        let now = at(2026, 6, 9, 19, 0);
        let got = next_reset("6:13 PM", now).unwrap();
        assert_eq!(
            got,
            at(2026, 6, 10, 18, 13),
            "past 12-hour time rolls to tomorrow"
        );
    }

    #[test]
    fn next_reset_12h_am_midnight_hour() {
        // 12:23 AM is 00:23 — the 12-hour edge chrono gets right via %I/%p.
        let now = at(2026, 6, 9, 23, 0);
        let got = next_reset("12:23 AM", now).unwrap();
        assert_eq!(got, at(2026, 6, 10, 0, 23), "12:23 AM is 00:23 next day");
    }

    #[test]
    fn next_reset_month_name_datetime_used_directly() {
        // Codex also emits a full month-name date-time with an ordinal day:
        // "Jun 10th, 2026 12:23 AM". Absolute — `now` is ignored.
        let now = at(2026, 6, 9, 10, 0);
        let got = next_reset("Jun 10th, 2026 12:23 AM", now).unwrap();
        assert_eq!(
            got,
            at(2026, 6, 10, 0, 23),
            "month-name date-time resolves to its exact local instant"
        );
    }

    #[test]
    fn next_reset_24h_still_parses_without_ampm() {
        // The AM/PM path must not shadow a bare 24-hour hint from other adapters.
        let now = at(2026, 6, 9, 10, 0);
        assert_eq!(next_reset("18:13", now).unwrap(), at(2026, 6, 9, 18, 13));
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
