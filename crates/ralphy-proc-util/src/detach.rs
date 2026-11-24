//! Start a process with no console and outside the Windows job of its parent,
//! so it outlives the shell, ssh session or launcher that started it.
//!
//! `DETACHED_PROCESS` alone does not do this: a child inherits its parent's
//! job, and when the job's owner ends the job, every process in it ends with
//! no line in the daemon log. Measured 2026-10-01: a daemon restarted from an
//! agent's shell tool sat in that tool's job and died with it.

use std::io;
use std::process::Child;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// Where a detached process ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Outside the job of its parent, as asked.
    OutsideJob,
    /// Inside it: the job refuses breakaway, so it ends when that job ends.
    InsideJob,
}

/// Start a process through `spawn`, which gets the creation flags: no console
/// and outside the job, or, when the job refuses breakaway, inside it.
///
/// # Errors
/// The spawn's own error, other than a refused breakaway.
pub fn spawn_detached(spawn: impl Fn(u32) -> io::Result<Child>) -> io::Result<(Child, Placement)> {
    let detached = CREATE_NO_WINDOW | DETACHED_PROCESS;
    match spawn(detached | CREATE_BREAKAWAY_FROM_JOB) {
        Err(e) if breakaway_refused(&e) => Ok((spawn(detached)?, Placement::InsideJob)),
        spawned => Ok((spawned?, Placement::OutsideJob)),
    }
}

/// A job that does not allow breakaway makes `CreateProcess` fail with
/// `ERROR_ACCESS_DENIED`; any other error is a real spawn failure.
pub fn breakaway_refused(error: &io::Error) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    error.raw_os_error() == Some(ERROR_ACCESS_DENIED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_access_denied_means_the_job_refused_breakaway() {
        assert!(breakaway_refused(&io::Error::from_raw_os_error(5)));
        assert!(!breakaway_refused(&io::Error::from_raw_os_error(2)));
    }
}
