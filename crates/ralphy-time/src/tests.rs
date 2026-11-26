use super::*;

/// Unix seconds, nanoseconds and offset seconds.
pub(crate) type Parts = (i64, i32, i32);

/// One instant of the table and the bytes chrono 0.4.45 wrote for it.
pub(crate) struct Golden {
    pub secs: i64,
    pub nanos: i32,
    pub offset: i32,
    pub secs_z: &'static str,
    pub millis_z: &'static str,
    pub local: &'static str,
    pub utc: &'static str,
}

const fn g(
    (secs, nanos, offset): (i64, i32, i32),
    secs_z: &'static str,
    millis_z: &'static str,
    local: &'static str,
    utc: &'static str,
) -> Golden {
    Golden {
        secs,
        nanos,
        offset,
        secs_z,
        millis_z,
        local,
        utc,
    }
}

// Written once by chrono 0.4.45 (`to_rfc3339_opts(Secs | Millis, true)` on UTC,
// `to_rfc3339()` at the offset and on UTC), 2026-11-26.
#[rustfmt::skip]
pub(crate) const INSTANTS: &[Golden] = &[
    g((0, 0, 0), "1970-01-01T00:00:00Z", "1970-01-01T00:00:00.000Z",
        "1970-01-01T00:00:00+00:00", "1970-01-01T00:00:00+00:00"),
    g((951_827_696, 0, 0), "2000-02-29T12:34:56Z", "2000-02-29T12:34:56.000Z",
        "2000-02-29T12:34:56+00:00", "2000-02-29T12:34:56+00:00"),
    g((951_827_696, 100_000_000, 0), "2000-02-29T12:34:56Z", "2000-02-29T12:34:56.100Z",
        "2000-02-29T12:34:56.100+00:00", "2000-02-29T12:34:56.100+00:00"),
    g((1_764_162_000, 123_000_000, -10800), "2025-11-26T13:00:00Z", "2025-11-26T13:00:00.123Z",
        "2025-11-26T10:00:00.123-03:00", "2025-11-26T13:00:00.123+00:00"),
    g((1_764_162_000, 123_456_000, 20700), "2025-11-26T13:00:00Z", "2025-11-26T13:00:00.123Z",
        "2025-11-26T18:45:00.123456+05:45", "2025-11-26T13:00:00.123456+00:00"),
    g((1_764_162_000, 123_456_700, 50400), "2025-11-26T13:00:00Z", "2025-11-26T13:00:00.123Z",
        "2025-11-27T03:00:00.123456700+14:00", "2025-11-26T13:00:00.123456700+00:00"),
    g((1_764_162_000, 123_456_789, -43200), "2025-11-26T13:00:00Z", "2025-11-26T13:00:00.123Z",
        "2025-11-26T01:00:00.123456789-12:00", "2025-11-26T13:00:00.123456789+00:00"),
    g((1_764_162_000, 0, 0), "2025-11-26T13:00:00Z", "2025-11-26T13:00:00.000Z",
        "2025-11-26T13:00:00+00:00", "2025-11-26T13:00:00+00:00"),
    // The last second before São Paulo moved to summer time, and the first after.
    g((1_541_300_399, 500_000_000, -10800), "2018-11-04T02:59:59Z", "2018-11-04T02:59:59.500Z",
        "2018-11-03T23:59:59.500-03:00", "2018-11-04T02:59:59.500+00:00"),
    g((1_541_300_400, 0, -7200), "2018-11-04T03:00:00Z", "2018-11-04T03:00:00.000Z",
        "2018-11-04T01:00:00-02:00", "2018-11-04T03:00:00+00:00"),
    g((0, 1, 0), "1970-01-01T00:00:00Z", "1970-01-01T00:00:00.000Z",
        "1970-01-01T00:00:00.000000001+00:00", "1970-01-01T00:00:00.000000001+00:00"),
    g((1_764_162_000, 999_999_999, -10800), "2025-11-26T13:00:00Z", "2025-11-26T13:00:00.999Z",
        "2025-11-26T10:00:00.999999999-03:00", "2025-11-26T13:00:00.999999999+00:00"),
    g((951_827_696, 1_000, 20700), "2000-02-29T12:34:56Z", "2000-02-29T12:34:56.000Z",
        "2000-02-29T18:19:56.000001+05:45", "2000-02-29T12:34:56.000001+00:00"),
    g((951_818_400, 0, -43200), "2000-02-29T10:00:00Z", "2000-02-29T10:00:00.000Z",
        "2000-02-28T22:00:00-12:00", "2000-02-29T10:00:00+00:00"),
    g((0, 1_000_000, 0), "1970-01-01T00:00:00Z", "1970-01-01T00:00:00.001Z",
        "1970-01-01T00:00:00.001+00:00", "1970-01-01T00:00:00.001+00:00"),
    g((1_793_219_161, 43_039_700, -10800), "2026-10-28T20:26:01Z", "2026-10-28T20:26:01.043Z",
        "2026-10-28T17:26:01.043039700-03:00", "2026-10-28T20:26:01.043039700+00:00"),
];

/// RFC 3339 texts chrono 0.4.45 read, with the instant and offset it gave.
#[rustfmt::skip]
pub(crate) const RFC3339_ACCEPTED: &[(&str, (i64, i32, i32))] = &[
    ("2026-11-26T13:00:00Z", (1_795_698_000, 0, 0)),
    ("2026-11-26T13:00:00z", (1_795_698_000, 0, 0)),
    ("2026-11-26t13:00:00Z", (1_795_698_000, 0, 0)),
    ("2026-11-26 13:00:00Z", (1_795_698_000, 0, 0)),
    ("2026-11-26T13:00:00.1234567891Z", (1_795_698_000, 123_456_789, 0)),
    ("2026-11-26T13:00:00+00:00", (1_795_698_000, 0, 0)),
    ("2026-11-26T13:00:00-00:00", (1_795_698_000, 0, 0)),
    ("2026-11-26T10:00:00.123-03:00", (1_795_698_000, 123_000_000, -10800)),
    ("2026-11-26T13:00:00+05:45", (1_795_677_300, 0, 20700)),
    ("2018-11-04T00:30:00-02:00", (1_541_298_600, 0, -7200)),
    ("2000-02-29T23:59:59+14:00", (951_818_399, 0, 50400)),
];

/// RFC 3339 texts chrono 0.4.45 refused.
pub(crate) const RFC3339_REFUSED: &[&str] = &[
    "2026-11-26T13:00:00",
    "2026-11-26T24:00:00Z",
    "",
    "2026-02-29T13:00:00Z",
    "2026-11-26T13:00:00+0300",
    "2026-11-26T13:00:00.Z",
    "2026-11-26T13:00Z",
    "2026-11-26T13:00:00Z ",
    "2026-11-26T13:00:00+24:00",
    "26-11-26T13:00:00Z",
];

/// RFC 2822 texts and what chrono 0.4.45 gave: `None` is a refusal.
#[rustfmt::skip]
pub(crate) const RFC2822: &[(&str, Option<Parts>)] = &[
    ("Thu, 26 Nov 2026 13:00:00 +0000", Some((1_795_698_000, 0, 0))),
    ("Thu, 26 Nov 2026 10:00:00 -0300", Some((1_795_698_000, 0, -10800))),
    ("Thu, 26 Nov 2026 13:00:00 +0545", Some((1_795_677_300, 0, 20700))),
    ("Tue, 29 Feb 2000 23:59:59 +1400", Some((951_818_399, 0, 50400))),
    ("26 Nov 2026 13:00:00 GMT", Some((1_795_698_000, 0, 0))),
    ("Thu, 26 Nov 2026 13:00 +0000", Some((1_795_698_000, 0, 0))),
    ("Thu, 26 Nov 2026 13:00:00 UT", Some((1_795_698_000, 0, 0))),
    ("Thu, 26 Nov 2026 13:00:00 EST", Some((1_795_716_000, 0, -18000))),
    ("Wed, 26 Nov 2026 13:00:00 +0000", None),
    ("Thu, 26 Nov 2026 13:00:00", None),
    ("", None),
    ("2026-11-26T13:00:00Z", None),
];

pub(crate) fn instant((secs, nanos, offset): (i64, i32, i32)) -> (Timestamp, Offset) {
    let t = Timestamp::new(secs, nanos).expect("the table holds valid instants");
    (
        t,
        Offset::from_seconds(offset).expect("the table holds valid offsets"),
    )
}

#[test]
fn formats_write_the_golden_bytes() {
    for row in INSTANTS {
        let (t, offset) = instant((row.secs, row.nanos, row.offset));
        assert_eq!(secs_z(t), row.secs_z, "secs_z of {t}");
        assert_eq!(millis_z(t), row.millis_z, "millis_z of {t}");
        assert_eq!(rfc3339(t, offset), row.local, "rfc3339 of {t} at {offset}");
        assert_eq!(rfc3339(t, Offset::UTC), row.utc, "rfc3339 of {t} at UTC");
    }
}

#[test]
fn the_sao_paulo_rows_carry_the_offsets_of_the_zone() {
    let zone = jiff::tz::TimeZone::get("America/Sao_Paulo").expect("the bundled zone database");
    for row in INSTANTS
        .iter()
        .filter(|r| r.secs_z.starts_with("2018-11-04"))
    {
        let (t, offset) = instant((row.secs, row.nanos, row.offset));
        assert_eq!(zone.to_offset(t), offset, "São Paulo at {t}");
    }
}

#[test]
fn rfc3339_parses_the_golden_inputs() {
    for &(text, want) in RFC3339_ACCEPTED {
        assert_eq!(parse_rfc3339(text), Ok(instant(want)), "{text:?}");
    }
    for &text in RFC3339_REFUSED {
        assert!(parse_rfc3339(text).is_err(), "{text:?} must be refused");
    }
}

#[test]
fn a_leap_second_reads_as_the_59th_second() {
    let want = instant((1_795_698_059, 0, 0));
    assert_eq!(parse_rfc3339("2026-11-26T13:00:60Z"), Ok(want));
}

#[test]
fn rfc2822_parses_the_golden_inputs() {
    for &(text, want) in RFC2822 {
        assert_eq!(parse_rfc2822(text).ok(), want.map(instant), "{text:?}");
    }
}

#[test]
fn last_release_fixtures_parse() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/last-release.txt"
    );
    let text = std::fs::read_to_string(path).expect("the fixture file");
    let lines: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(lines.len(), 4, "one line per form");
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let [form, written, secs, nanos, offset] = fields[..] else {
            panic!("five fields in {line:?}");
        };
        let small = |f: &str| f.parse::<i32>().expect("a number field");
        let want = (
            secs.parse().expect("a number field"),
            small(nanos),
            small(offset),
        );
        let (t, at) = instant(want);
        assert_eq!(parse_rfc3339(written).map(|(t, _)| t), Ok(t), "{line:?}");
        let rewritten = match form {
            "secs_z" => secs_z(t),
            "millis_z" => millis_z(t),
            "rfc3339_local" | "rfc3339_utc" => rfc3339(t, at),
            other => panic!("unknown form {other:?}"),
        };
        assert_eq!(rewritten, written, "the {form} form keeps its bytes");
    }
}
