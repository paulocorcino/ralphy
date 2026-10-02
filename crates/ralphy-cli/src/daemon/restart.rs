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

/// How long to wait for the old daemon to release its port before starting the
/// new one. Generous: a wedged old process is worth reporting, not racing.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);
/// How long the new daemon has to answer before the restart is reported as
/// failed. `host add` reads `describe` right after the restart, and a daemon
/// that does not answer yet shows no socket (ADR-0067 amendment M3).
const START_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn restart() -> Result<()> {
    let exe = current_exe()?;
    let store = ralphy_daemon::auth::store_dir()?;
    let args = effective(&ralphy_daemon::pidfile::read_args_in(&store));
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
    let port = super::describe::port_from_args(&args);
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
fn started(answers: impl Fn() -> bool, timeout: Duration, poll: Duration) -> bool {
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

/// Restart the daemon through systemd when the recorded pid is the main process
/// of the unit autostart installs. A kill looks like a crash to the unit's
/// `Restart=on-failure`, so systemd would start a second daemon next to the one
/// this command starts (ADR-0056 §11). Returns whether systemd did the restart.
fn restarted_by_systemd(store: &Path) -> Result<bool> {
    let Some(pid) = ralphy_daemon::pidfile::read_in(store) else {
        return Ok(false);
    };
    if systemd_main_pid() != Some(pid) {
        return Ok(false);
    }
    let status = std::process::Command::new("systemctl")
        .args(["--user", "restart", UNIT_NAME])
        .status()
        .with_context(|| format!("running systemctl --user restart {UNIT_NAME}"))?;
    if !status.success() {
        bail!("systemctl --user restart {UNIT_NAME} failed ({status})");
    }
    Ok(true)
}

/// Restart the daemon through launchd when the recorded pid is the one launchd
/// runs for the autostart agent. The agent's `KeepAlive` relaunches a killed
/// daemon, and the relaunch then fails on the port the daemon this command
/// spawned holds, again every ten seconds. Measured on macOS 12.7.6 after a
/// `ralphy host add`: `runs = 4`, `last exit code = 1`, "Address already in
/// use" in `daemon.log`.
///
/// A daemon outside the agent while the agent is loaded is the state an older
/// restart left behind. It is ended first, so the agent gets the port back. A
/// missing record with the agent running is the other state it left: the
/// agent's daemon is the one to restart.
fn restarted_by_launchd(store: &Path) -> Result<bool> {
    let recorded = ralphy_daemon::pidfile::read_in(store);
    match launchd_plan(recorded, launchd_agent()) {
        LaunchdPlan::NotTheAgent => return Ok(false),
        LaunchdPlan::Kickstart => {}
        LaunchdPlan::StopThenKickstart => {
            if !stop(store)? {
                return Ok(false);
            }
        }
    }
    let argv = kickstart_argv(&current_uid()?);
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .status()
        .with_context(|| format!("running {}", argv.join(" ")))?;
    if !status.success() {
        bail!("{} failed ({status})", argv.join(" "));
    }
    Ok(true)
}

/// What a restart does about the launchd agent.
#[derive(Debug, PartialEq, Eq)]
enum LaunchdPlan {
    /// No agent is loaded: the daemon is not launchd's to restart.
    NotTheAgent,
    /// The recorded daemon is the agent's own process.
    Kickstart,
    /// The agent is loaded, but the recorded daemon runs outside it.
    StopThenKickstart,
}

/// `agent` is `None` when no agent is loaded, else the pid it runs, if any.
/// With no record, a running agent is still the daemon to restart; an agent
/// that runs nothing is not, so a restart of "a running daemon" starts none.
fn launchd_plan(recorded: Option<u32>, agent: Option<Option<u32>>) -> LaunchdPlan {
    match (recorded, agent) {
        (_, None) => LaunchdPlan::NotTheAgent,
        (None, Some(Some(_))) => LaunchdPlan::Kickstart,
        (None, Some(None)) => LaunchdPlan::NotTheAgent,
        (Some(pid), Some(Some(running))) if pid == running => LaunchdPlan::Kickstart,
        (Some(_), Some(_)) => LaunchdPlan::StopThenKickstart,
    }
}

/// `launchctl kickstart -k` ends the running instance and starts the agent
/// again, so launchd stays the daemon's parent.
#[cfg(any(target_os = "macos", test))]
fn kickstart_argv(uid: &str) -> Vec<String> {
    vec![
        "launchctl".to_string(),
        "kickstart".to_string(),
        "-k".to_string(),
        format!("gui/{uid}/{LAUNCHD_LABEL}"),
    ]
}

#[cfg(not(any(target_os = "macos", test)))]
fn kickstart_argv(_uid: &str) -> Vec<String> {
    unreachable!("launchd_agent() is None off macOS, so nothing asks for a kickstart")
}

/// The uid of the `gui/<uid>` domain the agent is loaded in: this user's.
#[cfg(target_os = "macos")]
fn current_uid() -> Result<String> {
    let out = std::process::Command::new("id")
        .arg("-u")
        .output()
        .context("running id -u")?;
    let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || uid.is_empty() {
        bail!("id -u did not print this user's uid");
    }
    Ok(uid)
}

#[cfg(not(target_os = "macos"))]
fn current_uid() -> Result<String> {
    bail!("a launchd agent exists only on macOS")
}

/// `None` when the agent is not loaded, else the pid launchd reports for it,
/// which is `None` while it is not running.
#[cfg(target_os = "macos")]
fn launchd_agent() -> Option<Option<u32>> {
    let out = match std::process::Command::new("launchctl")
        .args(["list", LAUNCHD_LABEL])
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            tracing::debug!(error = %e, "launchctl is not available");
            return None;
        }
    };
    if !out.status.success() {
        return None;
    }
    Some(parse_launchd_pid(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(not(target_os = "macos"))]
fn launchd_agent() -> Option<Option<u32>> {
    None
}

/// `launchctl list <label>` prints a plist-like dictionary with a
/// `"PID" = <n>;` line only while the agent runs.
#[cfg(any(target_os = "macos", test))]
fn parse_launchd_pid(text: &str) -> Option<u32> {
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("\"PID\" = ")?
            .strip_suffix(';')?
            .trim()
            .parse::<u32>()
            .ok()
    })
}

/// The main pid systemd reports for the unit, or `None` when there is no such
/// running unit.
#[cfg(target_os = "linux")]
fn systemd_main_pid() -> Option<u32> {
    let out = match std::process::Command::new("systemctl")
        .args(["--user", "show", "-p", "MainPID", "--value", UNIT_NAME])
        .output()
    {
        Ok(out) => out,
        // No `systemctl`, or no user manager: the daemon is not a unit.
        Err(e) => {
            tracing::debug!(error = %e, "systemctl is not available");
            return None;
        }
    };
    parse_main_pid(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(target_os = "linux"))]
fn systemd_main_pid() -> Option<u32> {
    None
}

/// `systemctl show -p MainPID --value` prints the pid, and `0` for a unit that
/// is not running.
#[cfg(any(target_os = "linux", test))]
fn parse_main_pid(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().filter(|pid| *pid != 0)
}

fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("resolving this executable")?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

/// A path an operator can read: Windows canonicalization returns the
/// extended-length form, and printing it puts a prefix nobody typed in front of
/// every path this command reports.
fn readable(path: &Path) -> String {
    match path.display().to_string().strip_prefix(r"\\?\") {
        Some(plain) => plain.to_string(),
        None => path.display().to_string(),
    }
}

/// The recorded invocation, or a bare `daemon` when there is none — which is
/// what autostart runs, and what an older pid file implies.
pub(crate) fn effective(args: &[String]) -> Vec<String> {
    if args.is_empty() {
        vec!["daemon".to_string()]
    } else {
        args.to_vec()
    }
}

/// The production stop: real liveness, real identity, real killer.
fn stop(store: &Path) -> Result<bool> {
    stop_recorded(
        store,
        EXIT_TIMEOUT,
        ralphy_proc_util::pid::pid_is_alive,
        ralphy_proc_util::pid::exe_of_pid,
        ralphy_proc_util::kill_tree_by_pid,
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
                    .map(readable)
                    .unwrap_or_else(|| "nothing".to_string()),
                running
                    .as_deref()
                    .map(readable)
                    .unwrap_or_else(|| "nothing".to_string())
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
fn same_program(a: &Path, b: &Path) -> bool {
    match (a.file_name(), b.file_name()) {
        (Some(a), Some(b)) => {
            unparked(&a.to_string_lossy()).eq_ignore_ascii_case(&unparked(&b.to_string_lossy()))
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
fn unparked(name: &str) -> String {
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
    base.to_string()
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

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

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
    let detached = CREATE_NO_WINDOW | DETACHED_PROCESS;
    match spawn(detached | CREATE_BREAKAWAY_FROM_JOB) {
        Err(e) if breakaway_refused(&e) => {
            eprintln!(
                "warning: the daemon stays inside the job of the program that started it, \
                 so it stops when that program closes"
            );
            spawn(detached)
        }
        spawned => spawned,
    }
    .with_context(|| format!("spawning {} {}", readable(exe), args.join(" ")))
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
mod tests {
    use super::*;

    #[test]
    fn started_returns_once_the_daemon_answers() {
        let asked = std::cell::Cell::new(0);
        let answers = || {
            asked.set(asked.get() + 1);
            asked.get() >= 3
        };
        assert!(started(answers, Duration::from_secs(5), Duration::ZERO));
        assert_eq!(asked.get(), 3);
    }

    #[test]
    fn started_gives_up_after_the_timeout() {
        assert!(!started(
            || false,
            Duration::from_millis(30),
            Duration::from_millis(5)
        ));
    }

    /// A restart continues the log: appending to what the last daemon wrote,
    /// creating the file when there is none, never truncating it.
    #[test]
    fn the_restart_log_is_appended_never_truncated() {
        use std::io::Write;
        let dir = scratch("log");
        {
            let mut log = open_log(&dir).expect("creating the log");
            log.write_all(
                b"first daemon
",
            )
            .expect("writing");
        }
        {
            let mut log = open_log(&dir).expect("reopening the log");
            log.write_all(
                b"second daemon
",
            )
            .expect("writing");
        }
        let text = std::fs::read_to_string(dir.join("daemon.log")).expect("reading");
        assert_eq!(
            text,
            "first daemon
second daemon
"
        );
    }

    /// A restarted daemon must outlive the program that ran `daemon restart`,
    /// so it may not stay in that program's job. The test joins a job that
    /// allows breakaway (no kill-on-close, so the rest of the binary is not at
    /// risk) and checks where the detached child ends up.
    #[cfg(windows)]
    #[test]
    #[allow(unsafe_code, reason = "FFI: the job object calls of the test")]
    fn a_restarted_daemon_leaves_the_job_of_its_launcher() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        // SAFETY: plain Win32 calls on handles this test owns; the info struct
        // is a zeroed POD of the size passed.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        assert!(!job.is_null(), "creating a job object");
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        let set = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                u32::try_from(std::mem::size_of_val(&info)).expect("the struct size fits u32"),
            )
        };
        assert_ne!(set, FALSE, "allowing breakaway on the test job");
        let joined = unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) };
        assert_ne!(joined, FALSE, "putting the test process in the job");

        let store = scratch("job");
        let mut child = spawn_detached(
            Path::new("ping"),
            &["-n".into(), "30".into(), "127.0.0.1".into()],
            &store,
        )
        .expect("spawning the stand-in daemon");
        let mut in_job = FALSE;
        let asked = unsafe { IsProcessInJob(child.as_raw_handle(), job, &mut in_job) };
        child.kill().expect("ending the stand-in daemon");
        child.wait().expect("reaping the stand-in daemon");
        unsafe { CloseHandle(job) };
        assert_ne!(asked, FALSE, "asking whether the child is in the job");
        assert_eq!(
            in_job, FALSE,
            "the detached daemon stayed in its launcher's job"
        );
    }

    #[cfg(windows)]
    #[test]
    fn only_access_denied_means_the_job_refused_breakaway() {
        assert!(breakaway_refused(&std::io::Error::from_raw_os_error(5)));
        assert!(!breakaway_refused(&std::io::Error::from_raw_os_error(2)));
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ralphy-restart-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn ours() -> PathBuf {
        PathBuf::from("/opt/ralphy/ralphy")
    }

    fn record(dir: &Path, pid: u32) {
        ralphy_daemon::pidfile::write_in(dir, pid, &ours(), &[]).expect("write");
    }

    #[test]
    fn a_unit_that_is_not_running_has_no_main_pid() {
        assert_eq!(
            parse_main_pid(
                "4242
"
            ),
            Some(4242)
        );
        assert_eq!(
            parse_main_pid(
                "0
"
            ),
            None,
            "systemd prints 0 for a stopped unit; no daemon has pid 0"
        );
        assert_eq!(parse_main_pid(""), None);
        assert_eq!(parse_main_pid("Failed to connect to bus"), None);
    }

    #[test]
    fn a_launchd_agent_reports_its_pid_only_while_it_runs() {
        // `launchctl list dev.ralphy.daemon` on macOS 12.7.6, running and not.
        let running = "{\n\t\"Label\" = \"dev.ralphy.daemon\";\n\t\"LastExitStatus\" = 256;\n\t\"PID\" = 3136;\n\t\"Program\" = \"/Users/user/.ralphy/bin/ralphy\";\n};\n";
        let stopped = "{\n\t\"Label\" = \"dev.ralphy.daemon\";\n\t\"LastExitStatus\" = 256;\n};\n";
        assert_eq!(parse_launchd_pid(running), Some(3136));
        assert_eq!(parse_launchd_pid(stopped), None);
        assert_eq!(parse_launchd_pid(""), None);
        assert_eq!(
            kickstart_argv("501"),
            vec!["launchctl", "kickstart", "-k", "gui/501/dev.ralphy.daemon"]
        );
    }

    #[test]
    fn a_daemon_outside_a_loaded_agent_is_stopped_before_the_kickstart() {
        assert_eq!(launchd_plan(Some(3136), None), LaunchdPlan::NotTheAgent);
        assert_eq!(
            launchd_plan(Some(3136), Some(Some(3136))),
            LaunchdPlan::Kickstart
        );
        // What a restart before this fix left: the agent retrying, not running.
        assert_eq!(
            launchd_plan(Some(3061), Some(None)),
            LaunchdPlan::StopThenKickstart
        );
        assert_eq!(
            launchd_plan(Some(3061), Some(Some(3136))),
            LaunchdPlan::StopThenKickstart
        );
        // The record was erased while the agent's daemon serves (rc.31 on the Mac).
        assert_eq!(launchd_plan(None, Some(Some(3307))), LaunchdPlan::Kickstart);
        assert_eq!(launchd_plan(None, Some(None)), LaunchdPlan::NotTheAgent);
    }

    #[test]
    fn an_unrecorded_invocation_falls_back_to_a_bare_daemon() {
        assert_eq!(effective(&[]), vec!["daemon".to_string()]);
        let recorded = vec![
            "daemon".to_string(),
            "--port".to_string(),
            "8080".to_string(),
        ];
        assert_eq!(
            effective(&recorded),
            recorded,
            "a daemon on 8080 must come back on 8080, not on the default"
        );
    }

    #[test]
    fn a_reported_path_carries_no_prefix_nobody_typed() {
        assert_eq!(
            readable(Path::new(r"\\?\C:\Program Files\ralphy\ralphy.exe")),
            r"C:\Program Files\ralphy\ralphy.exe"
        );
        assert_eq!(
            readable(Path::new("/usr/local/bin/ralphy")),
            "/usr/local/bin/ralphy"
        );
    }

    #[test]
    fn the_same_program_survives_a_move_and_a_spelling() {
        assert!(same_program(
            Path::new("/opt/ralphy/ralphy"),
            Path::new("/opt/ralphy/ralphy")
        ));
        assert!(
            same_program(
                Path::new(r"C:\bin\ralphy.exe"),
                Path::new(r"C:\bin\RALPHY.EXE")
            ),
            "canonicalization and case must not make a daemon look like a stranger"
        );
        assert!(!same_program(
            Path::new("/bin/ralphy"),
            Path::new("/usr/sbin/sshd")
        ));
    }

    #[test]
    fn a_daemon_whose_image_was_parked_is_still_that_daemon() {
        // The ordinary post-update state: `install` renamed the image the daemon
        // is still executing, so the record says `ralphy.exe` and the OS reports
        // `ralphy.exe.old`. Refusing there would leave the old daemon running
        // and clear the record — defeating the restart entirely.
        assert!(same_program(
            Path::new(r"C:\bin\ralphy.exe"),
            Path::new(r"C:\bin\ralphy.exe.old")
        ));
        assert!(
            same_program(
                Path::new("/usr/local/bin/ralphy"),
                Path::new("/usr/local/bin/ralphy.old.3")
            ),
            "a second install steps the park aside; it is still our binary"
        );
        // And the leniency does not reach past the park suffix.
        assert!(!same_program(
            Path::new("/bin/ralphy"),
            Path::new("/bin/sshd.old")
        ));
        assert_eq!(unparked("ralphy.exe"), "ralphy.exe");
        assert_eq!(unparked("ralphy.exe.old"), "ralphy.exe");
        assert_eq!(unparked("ralphy.old.12"), "ralphy");
    }

    #[test]
    fn no_pid_file_means_nothing_to_stop() {
        let dir = scratch("none");
        assert!(!stop_recorded(
            &dir,
            EXIT_TIMEOUT,
            |_| true,
            |_| Some(ours()),
            |_| unreachable!("nothing to kill")
        )
        .expect("no file is not an error"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_pid_is_cleared_rather_than_killed() {
        let dir = scratch("stale");
        record(&dir, 424_242);
        assert!(!stop_recorded(
            &dir,
            EXIT_TIMEOUT,
            |_| false,
            |_| None,
            |_| unreachable!("a dead pid is not killed")
        )
        .expect("a dead pid is not an error"));
        assert_eq!(
            ralphy_daemon::pidfile::read_in(&dir),
            None,
            "a ghost must not survive to confuse the next restart"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reused_pid_is_never_killed() {
        // The defect this guards: the file outlives a crash or a reboot, the
        // number comes back as something else, and a reader that checked only
        // liveness would end that process and its whole tree.
        let dir = scratch("reused");
        record(&dir, 4242);
        let stopped = stop_recorded(
            &dir,
            EXIT_TIMEOUT,
            |_| true,
            |_| Some(PathBuf::from("/usr/sbin/sshd")),
            |_| unreachable!("a stranger must never be killed"),
        )
        .expect("a reused pid is not an error");
        assert!(!stopped);
        assert_eq!(
            ralphy_daemon::pidfile::read_in(&dir),
            None,
            "and the worthless record is dropped"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unprovable_identity_refuses_rather_than_guessing() {
        // A pid file from an older build records no exe, and some platforms
        // cannot name a process's program at all. Neither is permission to kill.
        let dir = scratch("unprovable");
        std::fs::write(ralphy_daemon::pidfile::pid_path_in(&dir), "4242\ndaemon\n")
            .expect("legacy file");
        let err = stop_recorded(
            &dir,
            EXIT_TIMEOUT,
            |_| true,
            |_| Some(ours()),
            |_| unreachable!("nothing is killed on a guess"),
        )
        .expect_err("an unprovable identity must refuse");
        assert!(err.to_string().contains("cannot prove pid 4242"), "{err}");

        let err = stop_recorded(
            &dir,
            EXIT_TIMEOUT,
            |_| true,
            |_| None,
            |_| unreachable!("nothing is killed on a guess"),
        )
        .expect_err("a platform that cannot say must refuse too");
        assert!(err.to_string().contains("stop it yourself"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pid_that_never_dies_is_reported_not_ignored() {
        let dir = scratch("wedged");
        record(&dir, 424_242);
        let killed = std::sync::atomic::AtomicU32::new(0);
        // A short timeout: the branch under test is the give-up, not the wait.
        let err = stop_recorded(
            &dir,
            Duration::from_millis(50),
            |_| true,
            |_| Some(ours()),
            |_| {
                killed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        )
        .expect_err("a wedged daemon must be reported, not silently doubled");
        assert!(err.to_string().contains("did not exit"), "{err}");
        assert_eq!(killed.load(std::sync::atomic::Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_live_matching_pid_is_killed_once_and_the_file_cleared() {
        let dir = scratch("live");
        record(&dir, 4242);
        let killed = std::sync::atomic::AtomicU32::new(0);
        // Alive until the kill lands, gone after — the ordinary path.
        let stopped = stop_recorded(
            &dir,
            EXIT_TIMEOUT,
            |_| killed.load(std::sync::atomic::Ordering::SeqCst) == 0,
            |_| Some(ours()),
            |_| {
                killed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        )
        .expect("stop");
        assert!(stopped);
        assert_eq!(killed.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(ralphy_daemon::pidfile::read_in(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
