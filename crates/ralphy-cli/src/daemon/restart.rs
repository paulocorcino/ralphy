//! `ralphy daemon restart` — end the running daemon and start this binary in
//! its place (ADR-0056 §8).
//!
//! Replacing the binary is not enough: the resident daemon keeps executing the
//! image it started with, so an update the operator cannot observe is not an
//! update. The daemon records its pid and its invocation at startup
//! (`daemon.pid`); this reads them, ends that process tree, waits for it to
//! actually go, and spawns the new one detached with the same arguments —
//! started, never parented, the ownership model the peer nudge uses.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// How long to wait for the old daemon to release its port before starting the
/// new one. Generous: a wedged old process is worth reporting, not racing.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

pub(crate) fn restart() -> Result<()> {
    let store = ralphy_daemon::auth::store_dir()?;
    let args = ralphy_daemon::pidfile::read_args_in(&store);
    let stopped = stop_recorded(
        &store,
        EXIT_TIMEOUT,
        ralphy_proc_util::pid::pid_is_alive,
        ralphy_proc_util::kill_tree_by_pid,
    )?;
    if stopped {
        println!("stopped the running daemon");
    } else {
        println!("no daemon was running");
    }

    let exe = current_exe()?;
    let args = effective(&args);
    spawn_detached(&exe, &args)?;
    println!("started {} {}", readable(&exe), args.join(" "));
    Ok(())
}

/// Restart only a daemon that is actually running. Returns whether one was.
///
/// `ralphy update` uses this rather than [`restart`]: an update must not start a
/// daemon on a machine that had none, and it must not leave a stale one serving
/// the image it was replaced from.
pub(crate) fn restart_if_running() -> Result<bool> {
    let store = ralphy_daemon::auth::store_dir()?;
    let args = ralphy_daemon::pidfile::read_args_in(&store);
    let stopped = stop_recorded(
        &store,
        EXIT_TIMEOUT,
        ralphy_proc_util::pid::pid_is_alive,
        ralphy_proc_util::kill_tree_by_pid,
    )?;
    if !stopped {
        return Ok(false);
    }
    spawn_detached(&current_exe()?, &effective(&args))?;
    Ok(true)
}

fn current_exe() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe().context("resolving this executable")?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

/// A path an operator can read: Windows canonicalization returns the
/// extended-length form, and printing it puts a prefix nobody typed in front of
/// every path this command reports.
fn readable(path: &Path) -> String {
    let shown = path.display().to_string();
    match shown.strip_prefix(r"\\?\") {
        Some(plain) => plain.to_string(),
        None => shown,
    }
}

/// The recorded invocation, or a bare `daemon` when there is none — which is
/// what autostart runs, and what an older pid file implies.
fn effective(args: &[String]) -> Vec<String> {
    if args.is_empty() {
        vec!["daemon".to_string()]
    } else {
        args.to_vec()
    }
}

/// End whatever `daemon.pid` names, if it is alive. Returns whether anything was
/// stopped.
///
/// Both the liveness probe and the killer are injected. That is not ceremony: a
/// test that exercised the wait loop against the real killer would hand it the
/// test runner's own pid and take the suite down with it.
fn stop_recorded(
    store: &Path,
    timeout: Duration,
    alive: impl Fn(u32) -> bool,
    kill: impl Fn(u32),
) -> Result<bool> {
    let Some(pid) = ralphy_daemon::pidfile::read_in(store) else {
        return Ok(false);
    };
    if !alive(pid) {
        // A stale file names a process that is already gone; clear it rather
        // than leave the next restart reading a ghost.
        ralphy_daemon::pidfile::clear_in(store);
        return Ok(false);
    }

    kill(pid);

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !alive(pid) {
            ralphy_daemon::pidfile::clear_in(store);
            return Ok(true);
        }
        std::thread::sleep(POLL);
    }
    anyhow::bail!("daemon pid {pid} did not exit within {timeout:?}")
}

/// Start the daemon again with no console, null stdio, and the child dropped
/// unwaited — this command must not become the daemon's parent.
#[cfg(windows)]
fn spawn_detached(exe: &Path, args: &[String]) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .with_context(|| format!("spawning {} {}", exe.display(), args.join(" ")))?;
    Ok(())
}

#[cfg(not(windows))]
fn spawn_detached(exe: &Path, args: &[String]) -> Result<()> {
    use std::process::{Command, Stdio};

    Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawning {} {}", exe.display(), args.join(" ")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ralphy-restart-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
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
    fn no_pid_file_means_nothing_to_stop() {
        let dir = scratch("none");
        assert!(!stop_recorded(
            &dir,
            EXIT_TIMEOUT,
            |_| true,
            |_| unreachable!("nothing to kill")
        )
        .expect("no file is not an error"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_pid_is_cleared_rather_than_killed() {
        let dir = scratch("stale");
        ralphy_daemon::pidfile::write_in(&dir, 424_242, &[]).expect("write");
        assert!(!stop_recorded(
            &dir,
            EXIT_TIMEOUT,
            |_| false,
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
    fn a_pid_that_never_dies_is_reported_not_ignored() {
        let dir = scratch("wedged");
        ralphy_daemon::pidfile::write_in(&dir, 424_242, &[]).expect("write");
        let killed = std::sync::atomic::AtomicU32::new(0);
        // A short timeout: the branch under test is the give-up, not the wait.
        let err = stop_recorded(
            &dir,
            Duration::from_millis(50),
            |_| true,
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
    fn a_live_pid_is_killed_once_and_the_file_cleared() {
        let dir = scratch("live");
        ralphy_daemon::pidfile::write_in(&dir, 4242, &[]).expect("write");
        let killed = std::sync::atomic::AtomicU32::new(0);
        // Alive until the kill lands, gone after — the ordinary path.
        let stopped = stop_recorded(
            &dir,
            EXIT_TIMEOUT,
            |_| killed.load(std::sync::atomic::Ordering::SeqCst) == 0,
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
