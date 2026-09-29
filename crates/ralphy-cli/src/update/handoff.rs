//! `ralphy update --handoff <pid>`: the update the workbench asked for
//! (ADR-0056 §11).
//!
//! The daemon that started this process is still running when it starts, and
//! this process is in that daemon's process tree. So nothing here kills the
//! daemon: this process replaces the binary, prints the hand-over line, and
//! waits for the daemon to shut down by itself. Then it starts the new binary
//! and waits for it to record its pid. When that does not happen, it puts the
//! previous binary back and starts that one.
//!
//! Every line goes to `update.log` in the daemon store. Until the hand-over it
//! also goes to stdout, which the daemon reads. After the hand-over the daemon
//! is gone, and so is the reader.

use std::cell::RefCell;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ralphy_release::{Channel, Standing};

use super::{install_release, read_standing, Installed};
use crate::daemon::restart;

/// Longer than the daemon's own fallback exit (20 s in `api_update.rs`), so a
/// daemon that exits late is still waited for.
const OLD_EXIT_TIMEOUT: Duration = Duration::from_secs(45);
/// How long the new daemon gets to bind its port and record its pid.
const NEW_READY_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(200);

pub(super) fn run(daemon_pid: u32, channel: Channel, force: bool) -> Result<()> {
    let store = ralphy_daemon::auth::store_dir()?;
    let mut log = Log::open(&ralphy_daemon::release::update_log_path_in(&store))?;
    log.record(&format!(
        "update requested by the daemon with pid {daemon_pid}"
    ));
    match take_over(daemon_pid, channel, force, &store, &mut log) {
        Ok(()) => Ok(()),
        // Before the hand-over the daemon reads stderr too, so the error goes
        // back to the page through the ordinary return.
        Err(e) if log.attached() => {
            log.record(&format!("error: {e:#}"));
            Err(e)
        }
        Err(e) => {
            log.record(&format!("error: {e:#}"));
            // The daemon that read stderr has exited. Returning the error would
            // print it into a closed pipe; the log already holds it.
            std::process::exit(1);
        }
    }
}

fn take_over(
    daemon_pid: u32,
    channel: Channel,
    force: bool,
    store: &Path,
    log: &mut Log,
) -> Result<()> {
    // The arguments that come back are the ones this daemon recorded, so the
    // record must be this daemon's.
    if ralphy_daemon::pidfile::read_in(store) != Some(daemon_pid) {
        bail!("the daemon store does not record pid {daemon_pid} as the running daemon");
    }
    let args = restart::effective(&ralphy_daemon::pidfile::read_args_in(store));

    let (_, standing) = read_standing(channel, force)?;
    let Standing::Behind(gap) = standing else {
        bail!("there is no newer release to take");
    };
    let release = &gap[0];
    let Installed { dest, parked } = install_release(release, &mut |line| log.say(line))?;
    log.hand_over();

    let host = RealHost::new(&dest, &args, store);
    let recorded = || ralphy_daemon::pidfile::read_in(store);
    let start = || host.start();
    let kill = |pid| host.kill(pid);
    let roll_back = || roll_back(&dest, parked.as_deref());
    let probes = Probes {
        alive: &ralphy_proc_util::pid::pid_is_alive,
        recorded: &recorded,
        start: &start,
        kill: &kill,
        roll_back: &roll_back,
    };
    let outcome = restart_after(
        daemon_pid,
        &probes,
        &mut |line| log.record(line),
        OLD_EXIT_TIMEOUT,
        NEW_READY_TIMEOUT,
    )?;
    match outcome {
        Outcome::Started(pid) => {
            log.record(&format!("now on {} (daemon pid {pid})", release.tag_name));
            if let Some(parked) = parked {
                // On Windows the parked file is this process's own image, so it
                // cannot go until the next update.
                if std::fs::remove_file(&parked).is_err() {
                    log.record(&format!(
                        "the previous binary is parked at {}",
                        parked.display()
                    ));
                }
            }
            // Only now: a peer that fails to update must never cost this host
            // its daemon.
            #[cfg(windows)]
            super::wsl::update_peers(channel, &mut |line| log.record(line));
            Ok(())
        }
        Outcome::RolledBack(pid) => {
            bail!(
                "{} did not start; the previous binary is back (daemon pid {pid})",
                release.tag_name
            )
        }
    }
}

/// How the restart ended.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The new binary is serving, as this pid.
    Started(u32),
    /// The new binary did not come up; the previous one is serving, as this pid.
    RolledBack(u32),
}

/// What the restart does to processes and files. Injected, so a test never
/// kills or starts a process.
struct Probes<'a> {
    alive: &'a dyn Fn(u32) -> bool,
    /// The pid the daemon store records now.
    recorded: &'a dyn Fn() -> Option<u32>,
    /// Start the binary at the destination path; its pid.
    start: &'a dyn Fn() -> Result<u32>,
    kill: &'a dyn Fn(u32),
    /// Put the previous binary back at the destination path.
    roll_back: &'a dyn Fn() -> Result<()>,
}

/// Wait for `old` to exit, start the new binary, and wait for it to record its
/// pid. `serve` records the pid only after its listener is bound, so a recorded
/// new pid means the new daemon is ready.
fn restart_after(
    old: u32,
    host: &Probes<'_>,
    say: &mut dyn FnMut(&str),
    exit_timeout: Duration,
    ready_timeout: Duration,
) -> Result<Outcome> {
    if !wait_until(exit_timeout, || !(host.alive)(old)) {
        bail!(
            "the daemon (pid {old}) did not exit within {exit_timeout:?}; \
             the new binary starts at the next restart"
        );
    }
    say("the daemon has exited; starting the new binary");

    match (host.start)() {
        Ok(new) => {
            if wait_until(ready_timeout, || {
                (host.recorded)() == Some(new) && (host.alive)(new)
            }) {
                return Ok(Outcome::Started(new));
            }
            say(&format!(
                "the new daemon (pid {new}) did not record itself within {ready_timeout:?}"
            ));
            if (host.alive)(new) {
                (host.kill)(new);
            }
        }
        Err(e) => say(&format!("the new binary did not start: {e:#}")),
    }

    say("restoring the previous binary");
    (host.roll_back)().context("restoring the previous binary")?;
    let back = (host.start)().context("starting the previous binary again")?;
    Ok(Outcome::RolledBack(back))
}

/// Poll `done` until it holds or `timeout` passes. Checked once more at the
/// deadline, so a condition that became true during the last sleep counts.
fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL.min(timeout));
    }
}

/// The production start and kill.
struct RealHost<'a> {
    dest: &'a Path,
    args: &'a [String],
    store: &'a Path,
    /// The last daemon this process started, so a daemon that did not come up
    /// is ended by its own handle. A tree kill by pid would not reach it on
    /// Unix, where it leads no process group.
    started: RefCell<Option<std::process::Child>>,
}

impl<'a> RealHost<'a> {
    fn new(dest: &'a Path, args: &'a [String], store: &'a Path) -> Self {
        Self {
            dest,
            args,
            store,
            started: RefCell::new(None),
        }
    }

    fn start(&self) -> Result<u32> {
        let child = restart::spawn_detached(self.dest, self.args, self.store)?;
        let pid = child.id();
        *self.started.borrow_mut() = Some(child);
        Ok(pid)
    }

    fn kill(&self, pid: u32) {
        let Some(mut child) = self.started.borrow_mut().take() else {
            return;
        };
        if child.id() != pid {
            return;
        }
        if let Err(e) = child.kill() {
            tracing::warn!(pid, error = %e, "could not end the new daemon");
        }
        if let Err(e) = child.wait() {
            tracing::warn!(pid, error = %e, "could not reap the new daemon");
        }
    }
}

/// Move the new binary aside and put the parked one back at `dest`. A rename,
/// not a delete: on Windows a file that is still mapped can be renamed.
fn roll_back(dest: &Path, parked: Option<&Path>) -> Result<()> {
    let Some(parked) = parked else {
        bail!("there is no previous binary to restore");
    };
    let mut failed = dest.as_os_str().to_owned();
    failed.push(".failed");
    let failed = PathBuf::from(failed);
    if failed.exists() {
        std::fs::remove_file(&failed).with_context(|| format!("removing {}", failed.display()))?;
    }
    std::fs::rename(dest, &failed).with_context(|| format!("moving {} aside", dest.display()))?;
    std::fs::rename(parked, dest)
        .with_context(|| format!("putting {} back at {}", parked.display(), dest.display()))
}

/// The update's output: always the log file, and stdout until the hand-over.
struct Log {
    file: File,
    stdout: bool,
}

impl Log {
    fn open(path: &Path) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        Ok(Self { file, stdout: true })
    }

    fn attached(&self) -> bool {
        self.stdout
    }

    /// A line for the log and, while the daemon reads it, for stdout.
    fn say(&mut self, line: &str) {
        self.record(line);
        if self.stdout {
            let mut out = std::io::stdout().lock();
            if let Err(e) = writeln!(out, "{line}").and_then(|()| out.flush()) {
                // The reader is gone. Nothing more is written to stdout.
                self.stdout = false;
                self.record(&format!("stdout closed: {e}"));
            }
        }
    }

    /// A line for the log only.
    fn record(&mut self, line: &str) {
        if let Err(e) = writeln!(self.file, "{line}") {
            // The log is the only record after the hand-over; a failed write
            // there has nowhere else to go.
            tracing::warn!(error = %e, "could not write the update log");
        }
    }

    /// Tell the daemon the new binary is in place, then stop writing to stdout.
    fn hand_over(&mut self) {
        self.say(ralphy_daemon::release::HANDOFF_READY);
        self.stdout = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const SHORT: Duration = Duration::from_millis(50);

    /// A scripted host: `old` exits when asked, each start takes the next pid,
    /// and a started pid records itself only when `records` allows it.
    struct Script {
        old_exits: bool,
        start_fails_first: bool,
        records: &'static dyn Fn(u32) -> bool,
        starts: Cell<u32>,
        killed: Cell<Option<u32>>,
        rolled_back: Cell<bool>,
        current: Cell<Option<u32>>,
    }

    impl Script {
        fn new(records: &'static dyn Fn(u32) -> bool) -> Self {
            Self {
                old_exits: true,
                start_fails_first: false,
                records,
                starts: Cell::new(0),
                killed: Cell::new(None),
                rolled_back: Cell::new(false),
                current: Cell::new(None),
            }
        }

        fn run(&self) -> Result<Outcome> {
            let alive = |pid: u32| {
                if pid == 1 {
                    !self.old_exits
                } else {
                    self.killed.get() != Some(pid)
                }
            };
            let recorded = || self.current.get().filter(|pid| (self.records)(*pid));
            let start = || {
                let n = self.starts.get() + 1;
                self.starts.set(n);
                if self.start_fails_first && n == 1 {
                    bail!("spawn refused");
                }
                let pid = 100 + n;
                self.current.set(Some(pid));
                Ok(pid)
            };
            let kill = |pid: u32| self.killed.set(Some(pid));
            let roll_back = || {
                self.rolled_back.set(true);
                Ok(())
            };
            let probes = Probes {
                alive: &alive,
                recorded: &recorded,
                start: &start,
                kill: &kill,
                roll_back: &roll_back,
            };
            restart_after(1, &probes, &mut |_| {}, SHORT, SHORT)
        }
    }

    #[test]
    fn a_new_daemon_that_records_itself_is_the_end_of_the_update() {
        let script = Script::new(&|_| true);
        assert_eq!(script.run().expect("restart"), Outcome::Started(101));
        assert!(!script.rolled_back.get(), "a daemon that came up is kept");
        assert_eq!(script.killed.get(), None);
    }

    #[test]
    fn a_daemon_that_does_not_exit_is_reported_and_nothing_is_started() {
        let mut script = Script::new(&|_| true);
        script.old_exits = false;
        let err = script.run().expect_err("the old daemon is still serving");
        assert!(err.to_string().contains("did not exit"), "{err}");
        assert_eq!(
            script.starts.get(),
            0,
            "two daemons must not fight for the port"
        );
    }

    #[test]
    fn a_new_daemon_that_never_records_itself_is_ended_and_rolled_back() {
        // Only the second start, the restored binary, comes up.
        let script = Script::new(&|pid| pid == 102);
        assert_eq!(script.run().expect("rollback"), Outcome::RolledBack(102));
        assert_eq!(script.killed.get(), Some(101), "the failed daemon is ended");
        assert!(script.rolled_back.get());
    }

    #[test]
    fn a_new_binary_that_does_not_spawn_is_rolled_back_too() {
        let mut script = Script::new(&|_| true);
        script.start_fails_first = true;
        assert_eq!(script.run().expect("rollback"), Outcome::RolledBack(102));
        assert!(script.rolled_back.get());
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ralphy-handoff-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn a_roll_back_puts_the_parked_binary_back_and_keeps_the_failed_one_aside() {
        let dir = scratch("rollback");
        let dest = dir.join("ralphy");
        let parked = dir.join("ralphy.old");
        std::fs::write(&dest, "new").expect("new binary");
        std::fs::write(&parked, "old").expect("parked binary");
        roll_back(&dest, Some(&parked)).expect("roll back");
        assert_eq!(std::fs::read_to_string(&dest).expect("dest"), "old");
        assert_eq!(
            std::fs::read_to_string(dir.join("ralphy.failed")).expect("failed"),
            "new"
        );
        assert!(!parked.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_roll_back_with_nothing_parked_refuses() {
        let err = roll_back(Path::new("ralphy"), None).expect_err("nothing to restore");
        assert!(err.to_string().contains("no previous binary"), "{err}");
    }
}
