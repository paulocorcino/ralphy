//! The verdict on how a finished child ended, before any ladder reads its log
//! for a cause (the ADR-0023 amendment of 2026-11-01). It owns two rules: a
//! free-text detector judges only a run that failed, and the order of the stop
//! checks. Each adapter still passes its own detector; this module takes the
//! facts of the run, never the child (ADR-0002).

use crate::{HeadlessOutput, HeadlessRun};

/// How a finished child ended, as the caller observed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunFacts {
    /// The child exited on its own with a success status. A caller whose run
    /// must also leave an artifact (a plan) sets it to `false` when the
    /// artifact is missing.
    pub exited_cleanly: bool,
    /// A clock (the wall deadline or the idle watchdog) killed the child.
    pub timed_out: bool,
    /// The operator's stop killed the child.
    pub stopped: bool,
}

impl From<&HeadlessOutput> for RunFacts {
    fn from(out: &HeadlessOutput) -> Self {
        Self {
            exited_cleanly: out.exited_cleanly(),
            timed_out: out.timed_out,
            stopped: out.stopped,
        }
    }
}

impl From<&HeadlessRun> for RunFacts {
    fn from(run: &HeadlessRun) -> Self {
        Self {
            exited_cleanly: run.exited_cleanly,
            timed_out: run.timed_out,
            stopped: run.stopped,
        }
    }
}

/// Why a run stopped, in the order [`failed_run_stop`] checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStop {
    /// The operator's stop killed the child. Its log is cut, so no cause read
    /// from it is true.
    Stopped,
    /// The run failed and its log carries the adapter's login error.
    Auth,
    /// A clock killed the child.
    Timeout,
}

/// The stop a finished run reports, or `None` when its end names no stop.
/// Order: stopped, then auth, then timeout. `auth` reads `log` only when the
/// run did not exit cleanly: the log of an agent CLI holds what the agent read,
/// so a good run may quote a login error.
pub fn failed_run_stop(
    facts: &RunFacts,
    log: &str,
    auth: impl Fn(&str) -> bool,
) -> Option<RunStop> {
    if facts.stopped {
        return Some(RunStop::Stopped);
    }
    if !facts.exited_cleanly && auth(log) {
        return Some(RunStop::Auth);
    }
    facts.timed_out.then_some(RunStop::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTH_LINE: &str = "LOGIN-NEEDED\n";

    fn detector(log: &str) -> bool {
        log.contains("LOGIN-NEEDED")
    }

    #[test]
    fn every_run_fact_gets_its_stop() {
        use RunStop::{Auth, Stopped, Timeout};
        // (exited_cleanly, timed_out, stopped, auth line in log) -> stop
        let rows: [(bool, bool, bool, bool, Option<RunStop>); 16] = [
            (false, false, false, true, Some(Auth)),
            (false, false, true, true, Some(Stopped)),
            (false, true, false, true, Some(Auth)),
            (false, true, true, true, Some(Stopped)),
            (true, false, false, true, None),
            (true, false, true, true, Some(Stopped)),
            (true, true, false, true, Some(Timeout)),
            (true, true, true, true, Some(Stopped)),
            (false, false, false, false, None),
            (false, false, true, false, Some(Stopped)),
            (false, true, false, false, Some(Timeout)),
            (false, true, true, false, Some(Stopped)),
            (true, false, false, false, None),
            (true, false, true, false, Some(Stopped)),
            (true, true, false, false, Some(Timeout)),
            (true, true, true, false, Some(Stopped)),
        ];
        for (exited_cleanly, timed_out, stopped, auth_line, want) in rows {
            let facts = RunFacts {
                exited_cleanly,
                timed_out,
                stopped,
            };
            let log = if auth_line { AUTH_LINE } else { "all good\n" };
            assert_eq!(
                failed_run_stop(&facts, log, detector),
                want,
                "{facts:?} with auth line {auth_line}"
            );
        }
    }
}
