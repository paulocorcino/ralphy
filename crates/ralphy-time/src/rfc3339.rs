//! The RFC 3339 reader: it accepts and refuses what chrono's
//! `DateTime::parse_from_rfc3339` did, so a text the last release read
//! still reads the same.

use jiff::civil::{Date, DateTime, Time};

use crate::{Offset, ParseError, Timestamp};

/// Parses `2026-11-26T10:00:00.123-03:00` into its instant and its offset.
///
/// The date and the time may be split by `T`, `t` or a space; the offset is
/// `Z`, `z` or `±HH:MM` (U+2212 is a minus too); a fraction keeps nine digits and drops the rest.
/// A leap second (`:60`) reads as `:59`.
pub fn parse_rfc3339(text: &str) -> Result<(Timestamp, Offset), ParseError> {
    let mut cursor = Cursor {
        rest: text.as_bytes(),
    };
    let date = cursor.date()?;
    cursor.separator()?;
    let time = cursor.time()?;
    let offset = cursor.offset()?;
    if !cursor.rest.is_empty() {
        return Err(ParseError::new("text after the offset"));
    }
    let instant = offset
        .to_timestamp(DateTime::from_parts(date, time))
        .map_err(|e| out_of_range(&e))?;
    Ok((instant, offset))
}

struct Cursor<'a> {
    rest: &'a [u8],
}

impl Cursor<'_> {
    fn date(&mut self) -> Result<Date, ParseError> {
        let year = self.digits(4)?;
        self.byte(b'-')?;
        let month = self.digits(2)?;
        self.byte(b'-')?;
        let day = self.digits(2)?;
        Date::new(narrow(year)?, narrow(month)?, narrow(day)?).map_err(|e| out_of_range(&e))
    }

    fn separator(&mut self) -> Result<(), ParseError> {
        match self.next() {
            Some(b'T' | b't' | b' ') => Ok(()),
            _ => Err(ParseError::new("no `T` between the date and the time")),
        }
    }

    fn time(&mut self) -> Result<Time, ParseError> {
        let hour = self.digits(2)?;
        self.byte(b':')?;
        let minute = self.digits(2)?;
        self.byte(b':')?;
        let second = self.digits(2)?;
        let nanos = self.fraction()?;
        let second = if second == 60 { 59 } else { second };
        Time::new(narrow(hour)?, narrow(minute)?, narrow(second)?, nanos)
            .map_err(|e| out_of_range(&e))
    }

    fn fraction(&mut self) -> Result<i32, ParseError> {
        let Some((b'.', rest)) = self.rest.split_first() else {
            return Ok(0);
        };
        self.rest = rest;
        let count = self.rest.iter().take_while(|b| b.is_ascii_digit()).count();
        if count == 0 {
            return Err(ParseError::new("no digit after the decimal point"));
        }
        let (digits, rest) = self.rest.split_at(count);
        self.rest = rest;
        let nanos = digits
            .iter()
            .chain(std::iter::repeat(&b'0'))
            .take(9)
            .fold(0, |acc, b| acc * 10 + i32::from(b - b'0'));
        Ok(nanos)
    }

    fn offset(&mut self) -> Result<Offset, ParseError> {
        let sign = match self.next() {
            Some(b'Z' | b'z') => return Ok(Offset::UTC),
            Some(b'+') => 1,
            Some(b'-') => -1,
            Some(0xE2) => {
                self.rest = self
                    .rest
                    .strip_prefix(&[0x88, 0x92])
                    .ok_or_else(no_offset)?;
                -1
            }
            _ => return Err(no_offset()),
        };
        let hours = self.digits(2)?;
        self.byte(b':')?;
        let minutes = self.digits(2)?;
        if hours >= 24 || minutes >= 60 {
            return Err(ParseError::new("offset out of range"));
        }
        Offset::from_seconds(sign * (hours * 3600 + minutes * 60)).map_err(|e| out_of_range(&e))
    }

    fn digits(&mut self, count: usize) -> Result<i32, ParseError> {
        let Some((digits, rest)) = self.rest.split_at_checked(count) else {
            return Err(ParseError::new("text too short"));
        };
        if !digits.iter().all(u8::is_ascii_digit) {
            return Err(ParseError::new("a digit expected"));
        }
        self.rest = rest;
        Ok(digits
            .iter()
            .fold(0, |acc, b| acc * 10 + i32::from(b - b'0')))
    }

    fn byte(&mut self, expected: u8) -> Result<(), ParseError> {
        match self.next() {
            Some(b) if b == expected => Ok(()),
            _ => Err(ParseError::new(format!(
                "`{}` expected",
                char::from(expected)
            ))),
        }
    }

    fn next(&mut self) -> Option<u8> {
        let (&first, rest) = self.rest.split_first()?;
        self.rest = rest;
        Some(first)
    }
}

fn narrow<T: TryFrom<i32, Error: std::fmt::Display>>(value: i32) -> Result<T, ParseError> {
    T::try_from(value).map_err(|error| ParseError::new(error.to_string()))
}

fn no_offset() -> ParseError {
    ParseError::new("no offset")
}

fn out_of_range(error: &jiff::Error) -> ParseError {
    ParseError::new(error.to_string())
}
