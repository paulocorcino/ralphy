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

/// Read the registration, write it, then decide. The read comes first: read
/// after the write, every install would look like a second one.
fn register(
    was_registered: impl FnOnce() -> Result<bool>,
    write: impl FnOnce() -> Result<()>,
    answers: impl FnOnce() -> bool,
    windows_elevated: bool,
) -> Result<AfterRegistration> {
    let was_registered = was_registered()?;
    write()?;
    Ok(after_registration(
        was_registered,
        answers(),
        windows_elevated,
    ))
}

/// Only Windows refuses to start from an elevated shell: root on Linux runs
/// `daemon install` over ssh for `host add`, and starts the daemon as before.
fn blocks_an_elevated_start(on_windows: bool, elevated: bool) -> bool {
    on_windows && elevated
}

// The registration starts a bare `daemon`, so the daemon it starts listens on
// the default port whatever `--port` this command got.
pub(crate) fn install() -> Result<()> {
    crate::install::warn_if_elevated();
    let store = ralphy_daemon::auth::store_dir()?;
    let port = ralphy_daemon::DEFAULT_PORT;
    let elevated =
        blocks_an_elevated_start(cfg!(windows), ralphy_proc_util::elevated::runs_elevated());
    let next = register(
        || {
            Ok(autostart::status()
                .context("reading the daemon autostart registration")?
                .registered)
        },
        autostart::install,
        || super::describe::answers(&store, port),
        elevated,
    )?;
    println!("registered daemon autostart");
    match next {
        AfterRegistration::AlreadyRegistered => {}
        AfterRegistration::AlreadyRunning => println!("the daemon is already running"),
        AfterRegistration::NotAsAdministrator => println!(
            "the daemon was not started from this administrator window; \
             it starts when you sign in again"
        ),
        AfterRegistration::Start => start_and_wait(&store, port)?,
    }
    Ok(())
}

/// Start the daemon and wait until it answers and has written its pid file.
/// `host add` runs `daemon restart` right after this, and a restart that finds
/// no pid file starts a second daemon. A daemon that never gets there (no name
/// yet, a port in use) is reported, not an error: the registration is in
/// place either way.
fn start_and_wait(store: &std::path::Path, port: u16) -> Result<()> {
    start(store)?;
    let up = || {
        super::describe::answers(store, port) && ralphy_daemon::pidfile::read_in(store).is_some()
    };
    if super::restart::started(up, START_TIMEOUT, POLL) {
        println!("started the daemon");
    } else {
        eprintln!(
            "warning: the daemon did not answer within {}s after the start: see {}",
            START_TIMEOUT.as_secs(),
            store.join("daemon.log").display()
        );
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
    let exe = ralphy_proc_util::exe::running_exe()?;
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

    /// A fake registration: read it, then write it, as `daemon install` does.
    fn install_on(registered: &std::cell::Cell<bool>) -> AfterRegistration {
        register(
            || Ok(registered.get()),
            || {
                registered.set(true);
                Ok(())
            },
            || false,
            false,
        )
        .expect("a fake registration does not fail")
    }

    #[test]
    fn a_second_install_does_not_start_the_daemon_again() {
        let registered = std::cell::Cell::new(false);
        assert_eq!(install_on(&registered), AfterRegistration::Start);
        assert_eq!(
            install_on(&registered),
            AfterRegistration::AlreadyRegistered
        );
    }

    #[test]
    fn only_windows_refuses_an_elevated_start() {
        assert!(blocks_an_elevated_start(true, true));
        assert!(
            !blocks_an_elevated_start(false, true),
            "root on Linux starts it"
        );
        assert!(!blocks_an_elevated_start(true, false));
    }

    #[test]
    fn an_elevated_windows_shell_registers_but_does_not_start() {
        assert_eq!(
            after_registration(false, false, true),
            AfterRegistration::NotAsAdministrator
        );
    }
}
