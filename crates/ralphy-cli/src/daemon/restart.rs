//! `ralphy daemon restart` — end the running daemon and start the binary that
//! replaced it (ADR-0056 §8).
//!
//! Replacing the binary is not enough: the resident daemon keeps executing the
//! image it started with, so an update the operator cannot observe is not an
//! update. Two things here are easy to get wrong and are gotten right on
//! purpose.
//!
//! **Which binary comes back.** The caller passes the path it just wrote. This
//! command must not re-derive it from `current_exe()`, because on Linux that
//! reads `/proc/self/exe`, which follows the rename the replacement performed
//! and then the unlink — it resolves to `…/ralphy.old (deleted)`, and the spawn
//! fails after the old daemon has already been killed.
//!
//! **Which process gets killed.** The pid file outlives a crash and a reboot, so
//! a recorded pid that is alive is not necessarily the daemon: the number gets
//! reused. Nothing is killed unless the program that pid is running matches the
//! program the file recorded.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ralphy_daemon::autostart::{LAUNCHD_LABEL, UNIT_NAME};

mod service;
use service::{restarted_by_launchd, restarted_by_systemd};

/// How long to wait for the old daemon to release its port before starting the
/// new one. Generous: a wedged old process is worth reporting, not racing.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);
/// How long the new daemon has to answer before the restart is reported as
/// failed. `host add` reads `describe` right after the restart, and a daemon
/// that does not answer yet shows no socket (ADR-0067 amendment M3).
const START_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn restart(yes: bool) -> Result<()> {
    let exe = current_exe()?;
    let store = ralphy_daemon::auth::store_dir()?;
    let args = effective(&ralphy_daemon::pidfile::read_args_in(&store));
    let port = super::describe::port_from_args(&args);
    if !super::confirm::confirm_with_daemon(&store, port, yes)? {
        return Ok(());
    }
    if restarted_by_systemd(&store)? {
        println!("restarted {UNIT_NAME}");
    } else if restarted_by_launchd(&store)? {
        println!("restarted {LAUNCHD_LABEL}");
    } else {
        if stop(&store)? {
            println!("stopped the running daemon");
        } else {
            println!("no daemon was running");
        }
        spawn_detached(&exe, &args, &store)?;
        println!("started {} {}", readable(&exe), args.join(" "));
    }
    if !started(
        || super::describe::answers(&store, port),
        START_TIMEOUT,
        POLL,
    ) {
        bail!(
            "the daemon did not answer within {}s after the restart: see {}",
            START_TIMEOUT.as_secs(),
            readable(&store.join("daemon.log"))
        );
    }
    Ok(())
}

/// Whether `answers` turns true before `timeout` ends, asked every `poll`.
pub(crate) fn started(answers: impl Fn() -> bool, timeout: Duration, poll: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if answers() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(poll);
    }
}

/// Restart only a daemon that is actually running, bringing back `exe`.
///
/// `ralphy update` uses this rather than [`restart`] for two reasons: an update
/// must not start a daemon on a machine that had none, and the binary it just
/// wrote is the one that must come back — see the module note on
/// `current_exe()`.
pub(crate) fn restart_if_running(exe: &Path) -> Result<bool> {
    let store = ralphy_daemon::auth::store_dir()?;
    if restarted_by_systemd(&store)? || restarted_by_launchd(&store)? {
        return Ok(true);
    }
    let args = ralphy_daemon::pidfile::read_args_in(&store);
    if !stop(&store)? {
        return Ok(false);
    }
    spawn_detached(exe, &effective(&args), &store)?;
    Ok(true)
}

fn current_exe() -> Result<PathBuf> {
    ralphy_proc_util::exe::running_exe()
}

/// A path an operator can read: Windows canonicalization returns the
/// extended-length form, and printing it puts a prefix nobody typed in front of
/// every path this command reports.
fn readable(path: &Path) -> String {
    match path.display().to_string().strip_prefix(r"\\?\") {
        Some(plain) => plain.to_owned(),
        None => path.display().to_string(),
    }
}

/// The recorded invocation, or a bare `daemon` when there is none — which is
/// what autostart runs, and what an older pid file implies.
pub(crate) fn effective(args: &[String]) -> Vec<String> {
    if args.is_empty() {
        vec!["daemon".to_owned()]
    } else {
        args.to_vec()
    }
}

/// The production stop: real liveness, real identity, real killer.
pub(crate) fn stop(store: &Path) -> Result<bool> {
    stop_recorded(
        store,
        EXIT_TIMEOUT,
        ralphy_proc_util::pid::pid_is_alive,
        ralphy_proc_util::pid::exe_of_pid,
        |pid| {
            ralphy_proc_util::kill_tree_by_pid(pid);
            // A daemon spawned by `spawn_detached` stays in its caller's process
            // group, so the group signal above does not reach it. The pid is
            // proven to be the daemon before this runs.
            #[cfg(unix)]
            ralphy_proc_util::pid::kill_pid(pid);
        },
    )
}

/// End whatever `daemon.pid` names, if it is alive *and* provably the daemon.
/// Returns whether anything was stopped.
///
/// The probes and the killer are injected. That is not ceremony: a test that
/// exercised the wait loop against the real killer would hand it the test
/// runner's own pid and take the suite down with it.
fn stop_recorded(
    store: &Path,
    timeout: Duration,
    alive: impl Fn(u32) -> bool,
    exe_of: impl Fn(u32) -> Option<PathBuf>,
    kill: impl Fn(u32),
) -> Result<bool> {
    let Some(pid) = ralphy_daemon::pidfile::read_in(store) else {
        return Ok(false);
    };
    if !alive(pid) {
        // A stale file names a process that is already gone; clear it rather
        // than leave the next restart reading a ghost.
        ralphy_daemon::pidfile::clear_own_in(store, pid);
        return Ok(false);
    }

    // Liveness is not identity. The file survives a crash and a reboot, and the
    // number is then very likely alive again as something unrelated.
    let recorded = ralphy_daemon::pidfile::read_exe_in(store);
    let running = exe_of(pid);
    match (recorded.as_deref(), running.as_deref()) {
        (Some(recorded), Some(running)) if same_program(recorded, running) => {}
        (Some(recorded), Some(running)) => {
            // Proven *not* ours: the pid was reused. The record is worthless.
            tracing::debug!(
                recorded = %recorded.display(),
                running = %running.display(),
                "the recorded pid belongs to another program now"
            );
            ralphy_daemon::pidfile::clear_own_in(store, pid);
            return Ok(false);
        }
        _ => {
            bail!(
                "cannot prove pid {pid} is the daemon (the record names {}, the process says {}); \
                 stop it yourself and start it again",
                recorded
                    .as_deref()
                    .map_or_else(|| "nothing".to_owned(), readable),
                running
                    .as_deref()
                    .map_or_else(|| "nothing".to_owned(), readable)
            );
        }
    }

    kill(pid);

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !alive(pid) {
            ralphy_daemon::pidfile::clear_own_in(store, pid);
            return Ok(true);
        }
        std::thread::sleep(POLL);
    }
    bail!("daemon pid {pid} did not exit within {timeout:?}")
}

/// Whether two paths name the same program. Compared by file name: the record is
/// written before a replacement and read after one, and Windows and Unix
/// disagree on canonicalization (extended-length prefixes, resolved symlinks).
/// The question is "the same program", not "the same spelling".
pub(crate) fn same_program(a: &Path, b: &Path) -> bool {
    match (a.file_name(), b.file_name()) {
        (Some(a), Some(b)) => {
            let name = |n: &std::ffi::OsStr| {
                let n = n.to_string_lossy();
                // Linux reports an image whose file was replaced or unlinked as
                // `<path> (deleted)`: the state after `mv` over the binary.
                unparked(n.strip_suffix(" (deleted)").unwrap_or(&n))
            };
            name(a).eq_ignore_ascii_case(&name(b))
        }
        _ => false,
    }
}

/// Drop the `.old` (or `.old.N`) suffix `install` adds when it parks a binary.
///
/// This is the ordinary post-update state, not an edge case: replacing the
/// binary renames the image the daemon is still executing, so the record says
/// `ralphy.exe` while the OS reports `ralphy.exe.old`. Refusing on that would
/// leave the old daemon running and the record cleared — defeating the restart
/// the update exists to perform.
pub(crate) fn unparked(name: &str) -> String {
    let mut base = name;
    while let Some((head, tail)) = base.rsplit_once('.') {
        let is_park = tail.eq_ignore_ascii_case("old")
            || (!tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()));
        // A bare number is only a park suffix when an `.old` sits behind it.
        if tail.eq_ignore_ascii_case("old") {
            base = head;
            continue;
        }
        if is_park && head.to_ascii_lowercase().ends_with(".old") {
            base = head;
            continue;
        }
        break;
    }
    base.to_owned()
}

/// The file a restarted daemon logs to: `daemon.log` in the store, the same
/// name the Windows logon task and the launchd agent append to (ADR-0032). Opened
/// for APPEND, so a restart continues the log instead of truncating it.
fn open_log(store: &Path) -> Result<std::fs::File> {
    let path = store.join("daemon.log");
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", readable(&path)))
}

/// stdout and stderr for the restarted daemon: the log, or — when it cannot be
/// opened — nothing, said out loud. A log that cannot be written is no reason
/// to leave the operator without a daemon.
fn log_stdio(store: &Path) -> (std::process::Stdio, std::process::Stdio) {
    use std::process::Stdio;
    match open_log(store).and_then(|out| {
        let err = out
            .try_clone()
            .context("sharing the daemon log with stderr")?;
        Ok((out, err))
    }) {
        Ok((out, err)) => (Stdio::from(out), Stdio::from(err)),
        Err(error) => {
            eprintln!("warning: the restarted daemon will not log: {error:#}");
            (Stdio::null(), Stdio::null())
        }
    }
}

/// Start the daemon again with no console, its output appended to the store's
/// `daemon.log`, and the child dropped unwaited — this command must not become
/// the daemon's parent. Returns the child, so a caller can end one that did not
/// come up; dropping it leaves the daemon running.
///
/// The daemon also leaves the Windows job of the program that ran this command.
/// `DETACHED_PROCESS` does not: a child inherits its parent's job, and when the
/// job's owner ends the job, every process in it is terminated with no line in
/// `daemon.log`. Measured 2026-10-01: a daemon restarted from an agent's shell
/// tool sat in that tool's job and died with it. IDE terminals and OpenSSH
/// sessions also run their commands in a job.
#[cfg(windows)]
pub(crate) fn spawn_detached(
    exe: &Path,
    args: &[String],
    store: &Path,
) -> Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    let spawn = |flags: u32| {
        let (stdout, stderr) = log_stdio(store);
        Command::new(exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .creation_flags(flags)
            .spawn()
    };
    spawn_outside_job("the daemon", spawn)
        .with_context(|| format!("spawning {} {}", readable(exe), args.join(" ")))
}

/// Start a process with no console and outside the job of this command, so it
/// outlives the shell or ssh session that ran it. `spawn` gets the creation
/// flags. A job that refuses breakaway gets the process inside it, said out
/// loud with `what` as the subject.
#[cfg(windows)]
pub(crate) fn spawn_outside_job(
    what: &str,
    spawn: impl Fn(u32) -> std::io::Result<std::process::Child>,
) -> std::io::Result<std::process::Child> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    // Otherwise the child also inherits this command's own stdout, and a
    // caller that reads it (a pipe, an SSH session) waits until the child ends.
    ralphy_proc_util::keep_std_handles_from_children();
    let detached = CREATE_NO_WINDOW | DETACHED_PROCESS;
    match spawn(detached | CREATE_BREAKAWAY_FROM_JOB) {
        Err(e) if breakaway_refused(&e) => {
            eprintln!(
                "warning: {what} stays inside the job of the program that started it, \
                 so it stops when that program closes"
            );
            spawn(detached)
        }
        spawned => spawned,
    }
}

/// A job that does not allow breakaway makes `CreateProcess` fail with
/// `ERROR_ACCESS_DENIED`; any other error is a real spawn failure.
#[cfg(windows)]
fn breakaway_refused(error: &std::io::Error) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    error.raw_os_error() == Some(ERROR_ACCESS_DENIED)
}

#[cfg(not(windows))]
pub(crate) fn spawn_detached(
    exe: &Path,
    args: &[String],
    store: &Path,
) -> Result<std::process::Child> {
    use std::process::{Command, Stdio};

    let (stdout, stderr) = log_stdio(store);
    Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .with_context(|| format!("spawning {} {}", readable(exe), args.join(" ")))
}

#[cfg(test)]
mod tests;
