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

/// How long to wait for the old daemon to release its port before starting the
/// new one. Generous: a wedged old process is worth reporting, not racing.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

pub(crate) fn restart() -> Result<()> {
    let exe = current_exe()?;
    let store = ralphy_daemon::auth::store_dir()?;
    let args = ralphy_daemon::pidfile::read_args_in(&store);
    if stop(&store)? {
        println!("stopped the running daemon");
    } else {
        println!("no daemon was running");
    }

    let args = effective(&args);
    spawn_detached(&exe, &args)?;
    println!("started {} {}", readable(&exe), args.join(" "));
    Ok(())
}

/// Restart only a daemon that is actually running, bringing back `exe`.
///
/// `ralphy update` uses this rather than [`restart`] for two reasons: an update
/// must not start a daemon on a machine that had none, and the binary it just
/// wrote is the one that must come back — see the module note on
/// `current_exe()`.
pub(crate) fn restart_if_running(exe: &Path) -> Result<bool> {
    let store = ralphy_daemon::auth::store_dir()?;
    let args = ralphy_daemon::pidfile::read_args_in(&store);
    if !stop(&store)? {
        return Ok(false);
    }
    spawn_detached(exe, &effective(&args))?;
    Ok(true)
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
fn effective(args: &[String]) -> Vec<String> {
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
        ralphy_daemon::pidfile::clear_in(store);
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
            ralphy_daemon::pidfile::clear_in(store);
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
            ralphy_daemon::pidfile::clear_in(store);
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
        .with_context(|| format!("spawning {} {}", readable(exe), args.join(" ")))?;
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
        .with_context(|| format!("spawning {} {}", readable(exe), args.join(" ")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
