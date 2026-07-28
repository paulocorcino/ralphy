//! The cooperative stop flag (docs/adr/0054): one process-wide bit meaning
//! "the operator asked this run to stop".
//!
//! **Why a process global rather than an injected port.** The flag has to be
//! readable at six places that share no seam: the queue loop
//! ([`crate::runner`]), the reset wait ([`crate::runner`]'s clock), the verify
//! gate ([`crate::verify`]), the resume loop inside the execute phase, the
//! headless child pump in `ralphy-adapter-support`, and the PTY pump in
//! `ralphy-agent-claude`. The last three sit on the far side of the ADR-0002
//! boundary and never see a `RunClock`, a `QueueConfig`, or anything else core
//! injects — so a port would reach two of the six sites and then have to
//! coexist with a global anyway, which is two mechanisms for one job.
//!
//! Threading an `Arc<AtomicBool>` instead would cross `QueueConfig` (which has
//! no `Default`), `IssueBudget` (which is `Copy`), Claude's own exec config, and
//! one builder per adapter — strictly more indirection for identical behaviour.
//! A `ralphy run` process drives exactly one run, so "this process was asked to
//! stop" is a process-wide fact and is modelled as one.
//!
//! It is also the shape a Ctrl-C handler requires: an async-signal handler may
//! touch an atomic and nothing else.
//!
//! **This module knows no path.** It is a bit and three functions; who sets it,
//! and from what file, is `ralphy-cli`'s business (the run-scoped sentinel under
//! the snapshot directory). Core must not learn where a sentinel lives.
//!
//! **Reading it is not what stops a run.** A stop must travel as a
//! [`crate::runner::StopReason`], because the runner's teardown hands the branch
//! back only when `stop.is_some()` — the `None` path force-checks-out the
//! original branch and would destroy the work the run had in flight.

use std::sync::atomic::{AtomicBool, Ordering};

/// The bit. `Relaxed` would do — there is no other state being published with
/// it — but `Release`/`Acquire` costs nothing on every platform Ralphy ships to
/// and keeps the pairing obvious to the next reader.
static REQUESTED: AtomicBool = AtomicBool::new(false);

/// Ask this process's run to stop. Idempotent, and safe from any thread — the
/// setter is the snapshot delivery worker, not the run thread.
pub fn request() {
    REQUESTED.store(true, Ordering::Release);
}

/// Has a stop been asked for? Called from poll loops at up to 20 Hz, so it must
/// stay a plain load — no locks, no allocation, no I/O.
pub fn requested() -> bool {
    REQUESTED.load(Ordering::Acquire)
}

/// Clear the bit. **Test-only in practice**: a run process never un-asks.
///
/// It is `pub` because the tests that need it live in other crates
/// (`ralphy-adapter-support` drives real children against this flag), and
/// because a leaked `true` is a genuine hazard: `cargo nextest` runs each test
/// in its own process, but `cargo test` shares one process per test BINARY, and
/// both gate this repo. A test that sets the flag and does not clear it would
/// silently stop every test that runs after it in the same binary — as flake,
/// not as an error. Hence the standing rule, stated here because this is where
/// someone will look: **stop tests live in their own `tests/*.rs` binary,
/// serialized behind a mutex, and clear the flag through a guard that survives
/// a panicking assertion.**
pub fn clear() {
    REQUESTED.store(false, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deliberately ONE test, and it restores the flag before it returns: this
    /// module compiles into the same binary as every other `ralphy-core` unit
    /// test, so anything left set here would leak into all of them under
    /// `cargo test`. The behavioural tests live in `tests/stop.rs`.
    #[test]
    fn the_flag_round_trips_and_clears() {
        assert!(!requested(), "the flag starts clear");
        request();
        assert!(requested());
        request();
        assert!(requested(), "requesting twice is idempotent");
        clear();
        assert!(!requested());
    }
}
