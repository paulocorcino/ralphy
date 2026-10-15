//! `ralphy daemon install`: register the autostart and, on the first
//! registration only, start the daemon (ADR-0056 §12). A machine that ran the
//! install expects a daemon now, not after the next logon; a machine that was
//! already registered keeps the daemon it has.

use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_daemon::autostart;

const START_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

/// What `daemon install` does after it writes the registration.
#[derive(Debug, PartialEq, Eq)]
enum AfterRegistration {
    /// The registration existed before: only the registration was written.
    AlreadyRegistered,
    /// A daemon answers already, so starting a second one would fail on its port.
    AlreadyRunning,
    /// An elevated Windows shell: a daemon started from it would run as
    /// administrator. The `Run` value starts it at the next sign-in instead.
    NotAsAdministrator,
    /// The first registration and no daemon: start one.
    Start,
}

fn after_registration(
    was_registered: bool,
    answers: bool,
    windows_elevated: bool,
) -> AfterRegistration {
    if was_registered {
        AfterRegistration::AlreadyRegistered
    } else if answers {
        AfterRegistration::AlreadyRunning
    } else if windows_elevated {
        AfterRegistration::NotAsAdministrator
    } else {
        AfterRegistration::Start
    }
}

pub(crate) fn install(port: u16) -> Result<()> {
    crate::install::warn_if_elevated();
    let was_registered = autostart::status()
        .context("reading the daemon autostart registration")?
        .registered;
    autostart::install()?;
    println!("registered daemon autostart");
    let store = ralphy_daemon::auth::store_dir()?;
    let windows_elevated = cfg!(windows) && ralphy_proc_util::elevated::runs_elevated();
    match after_registration(
        was_registered,
        super::describe::answers(&store, port),
        windows_elevated,
    ) {
        AfterRegistration::AlreadyRegistered => {}
        AfterRegistration::AlreadyRunning => println!("the daemon is already running"),
        AfterRegistration::NotAsAdministrator => println!(
            "the daemon was not started from this administrator window; \
             it starts when you sign in again"
        ),
        AfterRegistration::Start => {
            start(&store)?;
            // `host add` runs `daemon restart` right after this. A restart that
            // came before the daemon wrote its pid file would start a second one.
            if !super::restart::started(
                || super::describe::answers(&store, port),
                START_TIMEOUT,
                POLL,
            ) {
                anyhow::bail!(
                    "the daemon did not answer within {}s after the start: see {}",
                    START_TIMEOUT.as_secs(),
                    store.join("daemon.log").display()
                );
            }
            println!("started the daemon");
        }
    }
    Ok(())
}

/// The systemd unit is started by systemd, so it stays the daemon's parent and
/// `daemon restart` finds it (`systemctl --user enable --now` in two steps:
/// `enable` ran in the registration).
#[cfg(all(unix, not(target_os = "macos")))]
fn start(_store: &std::path::Path) -> Result<()> {
    let status = std::process::Command::new("systemctl")
        .args(["--user", "start", autostart::UNIT_NAME])
        .status()
        .with_context(|| format!("running systemctl --user start {}", autostart::UNIT_NAME))?;
    if !status.success() {
        anyhow::bail!(
            "systemctl --user start {} failed ({status})",
            autostart::UNIT_NAME
        );
    }
    Ok(())
}

/// `launchctl bootstrap` in the registration already started the agent: its
/// plist has `RunAtLoad`.
#[cfg(target_os = "macos")]
fn start(_store: &std::path::Path) -> Result<()> {
    Ok(())
}

/// The command the `Run` value starts at logon, started now: detached and
/// outside the caller's job object, so it outlives an ssh session.
#[cfg(windows)]
fn start(store: &std::path::Path) -> Result<()> {
    let exe = std::env::current_exe().context("resolving this executable")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    super::restart::spawn_detached(&exe, &["daemon".to_string()], store)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_registration_starts_a_daemon() {
        assert_eq!(
            after_registration(false, false, false),
            AfterRegistration::Start
        );
        assert_eq!(
            after_registration(true, false, false),
            AfterRegistration::AlreadyRegistered,
            "a second install only writes the registration again"
        );
        assert_eq!(
            after_registration(true, true, false),
            AfterRegistration::AlreadyRegistered
        );
        assert_eq!(
            after_registration(false, true, false),
            AfterRegistration::AlreadyRunning,
            "a daemon that answers is not started twice"
        );
    }

    #[test]
    fn an_elevated_windows_shell_registers_but_does_not_start() {
        assert_eq!(
            after_registration(false, false, true),
            AfterRegistration::NotAsAdministrator
        );
    }
}
