//! The timestamp forms Ralphy writes to disk and to the wire, and the two
//! parsers that read them back. The bytes are the ones chrono wrote before
//! the move to jiff (ADR-0096): a written form never changes.

use std::fmt;

pub use jiff::tz::Offset;
pub use jiff::Timestamp;

mod rfc3339;

pub use rfc3339::parse_rfc3339;

/// A text that is not a timestamp in the form a parser reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    reason: String,
}

impl ParseError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for ParseError {}

/// `2026-11-26T13:00:00Z`: UTC, whole seconds, the fraction cut off.
pub fn secs_z(t: Timestamp) -> String {
    format!("{}Z", civil_seconds(Offset::UTC, t))
}

/// `2026-11-26T13:00:00.123Z`: UTC, always three fraction digits, cut off.
pub fn millis_z(t: Timestamp) -> String {
    let millis = t.subsec_nanosecond() / 1_000_000;
    format!("{}.{millis:03}Z", civil_seconds(Offset::UTC, t))
}

/// `2026-11-26T10:00:00.123456789-03:00`: the civil time at `offset`, with
/// 0, 3, 6 or 9 fraction digits (the fewest that keep every nanosecond) and
/// a numeric offset, `+00:00` for UTC.
pub fn rfc3339(t: Timestamp, offset: Offset) -> String {
    let nanos = offset.to_datetime(t).subsec_nanosecond();
    format!(
        "{}{}{}",
        civil_seconds(offset, t),
        fraction(nanos),
        offset_text(offset)
    )
}

/// Parses an RFC 2822 date, as in a mail or an HTTP header.
pub fn parse_rfc2822(text: &str) -> Result<(Timestamp, Offset), ParseError> {
    let zoned = jiff::fmt::rfc2822::parse(text).map_err(|e| ParseError::new(e.to_string()))?;
    Ok((zoned.timestamp(), zoned.offset()))
}

fn civil_seconds(offset: Offset, t: Timestamp) -> String {
    offset
        .to_datetime(t)
        .strftime("%Y-%m-%dT%H:%M:%S")
        .to_string()
}

fn fraction(nanos: i32) -> String {
    if nanos == 0 {
        String::new()
    } else if nanos % 1_000_000 == 0 {
        format!(".{:03}", nanos / 1_000_000)
    } else if nanos % 1_000 == 0 {
        format!(".{:06}", nanos / 1_000)
    } else {
        format!(".{nanos:09}")
    }
}

fn offset_text(offset: Offset) -> String {
    let seconds = offset.seconds();
    let sign = if seconds < 0 { '-' } else { '+' };
    let minutes = seconds.unsigned_abs() / 60;
    format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests;
