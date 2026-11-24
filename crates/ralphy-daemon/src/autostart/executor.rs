//! The host executor of autostart: the only `#[cfg]`-gated part of the module.

use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;

use anyhow::{Context, Result};

#[cfg(target_os = "macos")]
use super::launchd_plist;
#[cfg(not(windows))]
use super::{render_install, render_query, render_uninstall, Platform};
#[cfg(windows)]
use super::{
    startup, turned_off, windows_shortcut, LAUNCHER_NAME, RUN_SUBKEY, SHORTCUT_NAME,
    STARTUP_APPROVED_SUBKEY, TASK_NAME,
};
#[cfg(all(unix, not(target_os = "macos")))]
use super::{systemd_is_enabled, systemd_unit};
use super::{AutostartSpec, LAUNCHD_LABEL, UNIT_NAME};

/// Whether the daemon's autostart registration currently exists, and on
/// Windows two more facts: the user turned it off in Task Manager, and a
/// registration of an earlier version is still there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AutostartStatus {
    pub registered: bool,
    pub turned_off: bool,
    pub old_registration: bool,
}

fn home_dir() -> Result<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .context("could not resolve a home directory for daemon autostart")?;
    Ok(PathBuf::from(home))
}

/// `<home>/.ralphy/daemon.log` — where the Windows launcher and launchd append
/// daemon output (the systemd path relies on the journal instead).
fn daemon_log_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".ralphy").join("daemon.log"))
}

/// `~/.config/systemd/user/ralphy-daemon.service`.
fn unit_path() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".config")
        .join("systemd")
        .join("user")
        .join(UNIT_NAME))
}

/// `~/Library/LaunchAgents/dev.ralphy.daemon.plist`.
fn launch_agent_path() -> Result<PathBuf> {
    Ok(home_dir()?
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
}

/// The canonicalized absolute path to the running binary, so the registered
/// autostart resolves `ralphy` regardless of the scheduler's stripped PATH.
/// Mirrors `schedule.rs::current_exe`.
fn current_exe() -> Result<PathBuf> {
    ralphy_proc_util::exe::running_exe()
}

/// The uid that owns this process — the launchd `gui/<uid>` domain.
#[cfg(unix)]
#[expect(unsafe_code, reason = "FFI: libc::getuid")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "one signature across systems: the non-unix arm has no uid"
)]
fn current_uid() -> Option<u32> {
    // SAFETY: `getuid` takes no arguments, cannot fail, and touches no memory.
    Some(unsafe { libc::getuid() })
}

#[cfg(not(unix))]
fn current_uid() -> Option<u32> {
    None
}

fn build_spec() -> Result<AutostartSpec> {
    // Read here, at install time, from the operator's own shell — the only
    // context that has it. See [`systemd_unit`].
    let path = std::env::var("PATH").ok().filter(|p| !p.is_empty());
    Ok(AutostartSpec {
        program: current_exe()?,
        log_path: daemon_log_path()?,
        wsl_distro: std::env::var("WSL_DISTRO_NAME")
            .ok()
            .filter(|d| !d.is_empty()),
        uid: current_uid(),
        plist_path: launch_agent_path()?,
        // Same reasoning as the distro: the operator's shell is the one place
        // their PATH — `~/.local/bin`, `~/.cargo/bin`, the vendor CLIs — is
        // assembled.
        path,
    })
}

/// Run `argv` (program + args), failing unless it exits `0` — unless
/// `tolerate_missing`, which accepts a non-zero exit too (the idempotent
/// uninstall path, where "already absent" is not an error).
fn run_argv(argv: &[String], tolerate_missing: bool) -> Result<()> {
    let (prog, rest) = argv.split_first().context("empty autostart command")?;
    let mut cmd = ProcCommand::new(prog);
    cmd.args(rest);
    crate::child_env::remove_secrets(&mut cmd);
    let status = cmd.status().with_context(|| format!("running {prog}"))?;
    if !status.success() && !tolerate_missing {
        anyhow::bail!("{prog} exited with {status}");
    }
    Ok(())
}

/// Run `argv` and report only whether it exited `0` — the shape the Windows
/// and launchd status probes use, because neither tool's text is a contract.
fn argv_succeeds(argv: &[String]) -> Result<bool> {
    let (prog, rest) = argv.split_first().context("empty autostart command")?;
    let mut cmd = ProcCommand::new(prog);
    cmd.args(rest);
    crate::child_env::remove_secrets(&mut cmd);
    let out = cmd.output().with_context(|| format!("running {prog}"))?;
    Ok(out.status.success())
}

/// Write `body` to `path`, creating the parent directory first.
fn write_registration(path: &Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

/// Remove `path` if it exists; an absent file is the idempotent no-op.
fn remove_registration(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

/// Write the Startup shortcut and delete the `Run` value of an earlier
/// version, so an upgraded machine does not start the daemon twice.
#[cfg(windows)]
pub fn install() -> Result<()> {
    let spec = build_spec()?;
    let shortcut = windows_shortcut(&spec.program, &spec.log_path, &startup::startup_dir()?);
    anyhow::ensure!(
        shortcut.target.is_file(),
        "{LAUNCHER_NAME} is missing beside {}; update Ralphy, then run this again",
        spec.program.display()
    );
    startup::write_shortcut(&shortcut)
        .context("could not write the daemon's shortcut in the Startup folder")?;
    startup::delete_value(RUN_SUBKEY, TASK_NAME)?;
    Ok(())
}

/// Delete the Startup shortcut and the `Run` value of an earlier version;
/// either one already gone is the idempotent no-op.
#[cfg(windows)]
pub fn uninstall() -> Result<()> {
    remove_registration(&startup::startup_dir()?.join(SHORTCUT_NAME))?;
    startup::delete_value(RUN_SUBKEY, TASK_NAME)?;
    Ok(())
}

#[cfg(windows)]
pub fn status() -> Result<AutostartStatus> {
    let approved = startup::read_value(STARTUP_APPROVED_SUBKEY, SHORTCUT_NAME)?;
    Ok(AutostartStatus {
        registered: startup::startup_dir()?.join(SHORTCUT_NAME).is_file(),
        turned_off: turned_off(approved.as_deref()),
        old_registration: startup::read_value(RUN_SUBKEY, TASK_NAME)?.is_some(),
    })
}

/// Whether `install` must load the launchd agent again. A loaded agent whose
/// plist did not change is left alone: `bootstrap` runs it at once
/// (`RunAtLoad`), so a reload would start a daemon the operator stopped on
/// purpose, and a second install only writes the registration (ADR-0056 §12).
pub fn launchd_reload_needed(on_disk: Option<&str>, wanted: &str, loaded: bool) -> bool {
    !(loaded && on_disk == Some(wanted))
}

#[cfg(target_os = "macos")]
pub fn install() -> Result<()> {
    let spec = build_spec()?;
    let plist = launchd_plist(&spec);
    let on_disk = std::fs::read_to_string(&spec.plist_path).ok();
    let loaded = argv_succeeds(&render_query(Platform::Launchd, &spec))?;
    if !launchd_reload_needed(on_disk.as_deref(), &plist, loaded) {
        return Ok(());
    }
    write_registration(&spec.plist_path, &plist)?;
    // launchd opens `StandardOutPath` itself and does not create its parent;
    // on a fresh account `~/.ralphy` may not exist yet and the agent would
    // fail to spawn with nothing logged anywhere.
    if let Some(parent) = spec.log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // `bootstrap` refuses a label that is already loaded, so a re-install
    // would fail where the Windows shortcut and `systemctl enable` are
    // idempotent. Unload first, tolerating "not loaded": the plist on disk is
    // fresh, and what comes back up is what it describes.
    run_argv(&render_uninstall(Platform::Launchd, &spec), true)?;
    run_argv(&render_install(Platform::Launchd, &spec), false).with_context(|| {
        format!(
            "could not register daemon autostart (bootstrapping {} into gui/{})",
            spec.plist_path.display(),
            spec.uid.map(|u| u.to_string()).unwrap_or_default()
        )
    })
}

#[cfg(target_os = "macos")]
pub fn uninstall() -> Result<()> {
    let spec = build_spec()?;
    // Tolerate "not loaded" so a second uninstall is a clean no-op.
    run_argv(&render_uninstall(Platform::Launchd, &spec), true)?;
    remove_registration(&spec.plist_path)
}

#[cfg(target_os = "macos")]
pub fn status() -> Result<AutostartStatus> {
    // `launchctl print` exit code only (0 = loaded) — same discipline as
    // `reg query`; its text is verbose and not a contract.
    Ok(AutostartStatus {
        registered: argv_succeeds(&render_query(Platform::Launchd, &build_spec()?))?,
        ..AutostartStatus::default()
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn install() -> Result<()> {
    let spec = build_spec()?;
    write_registration(&unit_path()?, &systemd_unit(&spec))?;
    // systemd reads a changed unit only after a reload. Whether `enable`
    // reloads when the unit is already enabled is not measured, and a
    // `daemon restart` right after a reinstall must start the new `ExecStart`
    // (ADR-0067 amendment D8), so the reload is explicit.
    run_argv(
        &[
            "systemctl".to_owned(),
            "--user".to_owned(),
            "daemon-reload".to_owned(),
        ],
        false,
    )?;
    run_argv(&render_install(Platform::Systemd, &spec), false)
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn uninstall() -> Result<()> {
    // Tolerate "unit not found" so a second uninstall is a clean no-op.
    run_argv(&render_uninstall(Platform::Systemd, &build_spec()?), true)?;
    remove_registration(&unit_path()?)
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn status() -> Result<AutostartStatus> {
    let argv = render_query(Platform::Systemd, &build_spec()?);
    let (prog, rest) = argv.split_first().context("empty autostart command")?;
    let mut cmd = ProcCommand::new(prog);
    cmd.args(rest);
    crate::child_env::remove_secrets(&mut cmd);
    let out = cmd.output().with_context(|| format!("running {prog}"))?;
    Ok(AutostartStatus {
        registered: systemd_is_enabled(&String::from_utf8_lossy(&out.stdout)),
        ..AutostartStatus::default()
    })
}
