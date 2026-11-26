//! chrono beside `ralphy-time` on the same inputs. This module goes when
//! chrono leaves the workspace.

use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};

use super::tests::{instant, INSTANTS, RFC2822, RFC3339_ACCEPTED, RFC3339_REFUSED};
use super::*;

fn chrono_instant(t: Timestamp) -> DateTime<Utc> {
    let nanos =
        u32::try_from(t.subsec_nanosecond()).expect("the table holds no instant before 1970");
    DateTime::from_timestamp(t.as_second(), nanos).expect("an instant chrono can hold")
}

fn from_chrono(d: DateTime<FixedOffset>) -> (Timestamp, Offset) {
    let nanos = i32::try_from(d.timestamp_subsec_nanos()).expect("no leap second in the inputs");
    instant((d.timestamp(), nanos, d.offset().local_minus_utc()))
}

#[test]
fn chrono_parity_formats() {
    for row in INSTANTS {
        let (t, offset) = instant((row.secs, row.nanos, row.offset));
        let utc = chrono_instant(t);
        let at = FixedOffset::east_opt(offset.seconds()).expect("a valid offset");
        assert_eq!(secs_z(t), utc.to_rfc3339_opts(SecondsFormat::Secs, true));
        assert_eq!(
            millis_z(t),
            utc.to_rfc3339_opts(SecondsFormat::Millis, true)
        );
        assert_eq!(rfc3339(t, offset), utc.with_timezone(&at).to_rfc3339());
        assert_eq!(rfc3339(t, Offset::UTC), utc.to_rfc3339());
    }
}

#[test]
fn chrono_parity_parses() {
    let rfc3339_inputs = RFC3339_ACCEPTED
        .iter()
        .map(|(text, _)| *text)
        .chain(RFC3339_REFUSED.iter().copied());
    for text in rfc3339_inputs {
        let chrono = DateTime::parse_from_rfc3339(text).ok().map(from_chrono);
        assert_eq!(parse_rfc3339(text).ok(), chrono, "{text:?}");
    }
    for &(text, _) in RFC2822 {
        let chrono = DateTime::parse_from_rfc2822(text).ok().map(from_chrono);
        assert_eq!(parse_rfc2822(text).ok(), chrono, "{text:?}");
    }
}

#[test]
fn chrono_parity_local_offset() {
    let now = Timestamp::now();
    let jiff = jiff::tz::TimeZone::system().to_offset(now);
    let chrono = chrono_instant(now)
        .with_timezone(&chrono::Local)
        .offset()
        .local_minus_utc();
    assert_eq!(jiff.seconds(), chrono);
}
