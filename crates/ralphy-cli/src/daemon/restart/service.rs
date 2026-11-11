//! Restart through the service manager that owns the daemon: systemd on Linux,
//! launchd on macOS.

use std::path::Path;

use anyhow::{bail, Context, Result};
#[cfg(any(target_os = "macos", test))]
use ralphy_daemon::autostart::LAUNCHD_LABEL;
use ralphy_daemon::autostart::UNIT_NAME;

use super::stop;

/// Restart the daemon through systemd when the recorded pid is the main process
/// of the unit autostart installs. A kill looks like a crash to the unit's
/// `Restart=on-failure`, so systemd would start a second daemon next to the one
/// this command starts (ADR-0056 §11). Returns whether systemd did the restart.
pub(super) fn restarted_by_systemd(store: &Path) -> Result<bool> {
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
pub(super) fn restarted_by_launchd(store: &Path) -> Result<bool> {
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

/// What launchd reports for the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchdAgent {
    NotLoaded,
    /// Loaded; `pid` is `None` while the agent is not running.
    #[cfg_attr(
        not(any(target_os = "macos", test)),
        expect(dead_code, reason = "only launchctl on macOS reports a loaded agent")
    )]
    Loaded {
        pid: Option<u32>,
    },
}

/// With no record, a running agent is still the daemon to restart; an agent
/// that runs nothing is not, so a restart of "a running daemon" starts none.
fn launchd_plan(recorded: Option<u32>, agent: LaunchdAgent) -> LaunchdPlan {
    match (recorded, agent) {
        (_, LaunchdAgent::NotLoaded) | (None, LaunchdAgent::Loaded { pid: None }) => {
            LaunchdPlan::NotTheAgent
        }
        (None, LaunchdAgent::Loaded { pid: Some(_) }) => LaunchdPlan::Kickstart,
        (Some(pid), LaunchdAgent::Loaded { pid: Some(running) }) if pid == running => {
            LaunchdPlan::Kickstart
        }
        (Some(_), LaunchdAgent::Loaded { .. }) => LaunchdPlan::StopThenKickstart,
    }
}

/// `launchctl kickstart -k` ends the running instance and starts the agent
/// again, so launchd stays the daemon's parent.
#[cfg(any(target_os = "macos", test))]
fn kickstart_argv(uid: &str) -> Vec<String> {
    vec![
        "launchctl".to_owned(),
        "kickstart".to_owned(),
        "-k".to_owned(),
        format!("gui/{uid}/{LAUNCHD_LABEL}"),
    ]
}

#[cfg(not(any(target_os = "macos", test)))]
fn kickstart_argv(_uid: &str) -> Vec<String> {
    unreachable!("no launchd agent is loaded off macOS, so nothing asks for a kickstart")
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

/// The agent's state, as `launchctl list` reports it.
#[cfg(target_os = "macos")]
fn launchd_agent() -> LaunchdAgent {
    let out = match std::process::Command::new("launchctl")
        .args(["list", LAUNCHD_LABEL])
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            tracing::debug!(error = %e, "launchctl is not available");
            return LaunchdAgent::NotLoaded;
        }
    };
    if !out.status.success() {
        return LaunchdAgent::NotLoaded;
    }
    LaunchdAgent::Loaded {
        pid: parse_launchd_pid(&String::from_utf8_lossy(&out.stdout)),
    }
}

#[cfg(not(target_os = "macos"))]
fn launchd_agent() -> LaunchdAgent {
    LaunchdAgent::NotLoaded
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

#[cfg(test)]
mod tests;
