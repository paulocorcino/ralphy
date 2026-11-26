# One crate owns the timestamp forms

Status: proposed
Kind: structural
Protects: recoverability, integrity of change

## Context

Issue #994: RUSTSEC-2026-0543 marks `chrono` unmaintained, and `deny.toml`
ignores the advisory since #993. Ralphy moves to `jiff`, which reads the
system time zone database and finds the local offset on Unix while threads
run.

Ralphy writes timestamps to disk and to the wire, and other code reads them
as strings: the daemon's spend rows, the attention list, the events
platform. Today chrono writes four forms:

- `2026-11-26T13:00:00Z` (`to_rfc3339_opts(Secs, true)`): audit, attention,
  the events envelope, pricing, release.
- `2026-11-26T13:00:00.123Z` (`Millis`): the daemon's probe.
- `2026-11-26T10:00:00.123456789-03:00` (`to_rfc3339()` on local time): run
  snapshots, the run lock, sync, hook lines.
- `2026-11-26T13:00:00.123456789+00:00` (`to_rfc3339()` on UTC): the ledger,
  usage scans, the usage route.

chrono writes 0, 3, 6 or 9 fraction digits. jiff's default writes the fewest
digits and writes `Z` for UTC, so a call written at each site would change
the bytes. chrono's RFC 3339 reader also differs from jiff's: it takes a
space or `t` for `T` and refuses an offset without a colon. The forms are
used in `ralphy-core`, the adapters, `ralphy-usage-scan`, `ralphy-pricing`,
`ralphy-release`, the daemon and the CLI; three of those crates do not
depend on `ralphy-core`.

## Decision

**D1.** `ralphy-time` is the one crate that writes the four forms and reads
RFC 3339 and RFC 2822 text: `secs_z`, `millis_z`, `rfc3339(Timestamp,
Offset)`, `parse_rfc3339` and `parse_rfc2822`. No other crate formats or
parses a timestamp with its own format string.

**D2.** `ralphy-time` is a leaf crate with one normal dependency, `jiff`.

**D3.** A written form never changes its bytes. Each form is pinned by
literal goldens that chrono wrote, and by a fixture of lines the last
release wrote (`crates/ralphy-time/tests/fixtures/last-release.txt`).

**D4.** `parse_rfc3339` accepts and refuses what chrono's
`parse_from_rfc3339` did. The one known difference: a leap second (`:60`)
reads as `:59`, because jiff cannot hold one.

**D5.** Callers use jiff types (`Timestamp`, `Offset`, `SignedDuration`,
`TimeZone`) for arithmetic and zones. Only the text forms go through
`ralphy-time`.

## Consequences

While chrono is still in the workspace, a `chrono_parity` test module checks
`ralphy-time` against chrono on the same table. It is removed together with
chrono; the goldens and the fixture stay.

## Considered options

- jiff calls written at each site: the chrono fraction rule would be
  written in four crates.
- The helpers in `ralphy-core`: `ralphy-usage-scan`, `ralphy-pricing` and
  `ralphy-release` do not depend on core.
- The `time` crate: `UtcOffset::current_local_offset` refuses on Unix while
  threads run, and the CLI and the daemon read the local time often.

## Compliance

- D1: not checked by code: reviewed in the pull request until chrono is
  gone; after that, no other crate can call chrono.
- D2: not checked by code: `cargo tree -p ralphy-time -e normal` shows it;
  no xtask test yet.
- D3: checked by `crates/ralphy-time/src/tests.rs`
  (`formats_write_the_golden_bytes`, `last_release_fixtures_parse`).
- D4: checked by `crates/ralphy-time/src/tests.rs`
  (`rfc3339_parses_the_golden_inputs`,
  `a_leap_second_reads_as_the_59th_second`).
- D5: not checked by code: reviewed in the pull request.
