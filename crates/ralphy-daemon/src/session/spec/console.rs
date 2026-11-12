//! The free console's launch specs: the platform shell, a startup command,
//! and the peer environment's WSL distro.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{SessionSpec, AGENT_OVERRIDE_ENV};

/// The platform's default interactive shell for the free console (issue #167).
/// Windows prefers `pwsh` (PowerShell 7), falling back to Windows PowerShell,
/// then `%ComSpec%`/`cmd.exe` (both always present, unlike `pwsh`) so the
/// console stays usable on a box without PowerShell 7. Off Windows, the user's
/// login shell (`$SHELL`), falling back to `/bin/sh`.
fn default_shell() -> OsString {
    if cfg!(windows) {
        ralphy_proc_util::locate_program("pwsh")
            .or_else(|| ralphy_proc_util::locate_program("powershell"))
            .map(Into::into)
            .or_else(|| std::env::var_os("ComSpec"))
            .unwrap_or_else(|| "cmd.exe".into())
    } else {
        std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into())
    }
}

/// The free console's working directory: the chosen repo's path, or the home
/// directory when none was chosen, or `.` when even the home directory cannot
/// be resolved (so a console still spawns rather than failing outright).
pub fn console_cwd(repo_path: Option<PathBuf>) -> PathBuf {
    repo_path
        .or_else(ralphy_proc_util::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The argv that makes `shell` run `command` and exit with it: the console IS
/// the command (an `htop` console ends when htop does), so nothing wraps it in
/// a second interactive shell. Dispatched on the shell's file stem, not the
/// platform, because [`default_shell`] falls through several programs on
/// Windows and a test override names none of them:
///
/// - `pwsh`/`powershell` → `-NoLogo -Command <command>`;
/// - `cmd` → `/c <command>`;
/// - anything else → `-lc <command>`. `-l` (login) is deliberate: a monitor
///   tool installed under `~/.local/bin` or by a version manager is on the
///   PATH only once the profile ran, and the operator wrote the command as
///   they would type it at their own prompt.
pub(super) fn shell_command_args(shell: &Path, command: &str) -> Vec<OsString> {
    // Split on BOTH separators rather than asking `Path` for the stem: on Unix a
    // backslash is not a separator, so `file_stem` of a Windows path is the
    // whole path and PowerShell would be read as a POSIX shell.
    let spelled = shell.to_string_lossy();
    let name = spelled.rsplit(['/', '\\']).next().unwrap_or_default();
    let stem = name
        .rsplit_once('.')
        .map_or(name, |(stem, _)| stem)
        .to_ascii_lowercase();
    match stem.as_str() {
        "pwsh" | "powershell" => vec!["-NoLogo".into(), "-Command".into(), command.into()],
        "cmd" => vec!["/c".into(), command.into()],
        _ => vec!["-lc".into(), command.into()],
    }
}

/// Build the launch spec for the free console (issue #167): the platform shell
/// in `cwd` — bare when `command` is `None`, else running `command` through
/// `shell_command_args` (the startup-command console) — unless
/// `RALPHY_DAEMON_AGENT_OVERRIDE` names a stand-in program instead (the same
/// test seam [`spec_for`](super::spec_for) uses; the override still receives the command argv).
pub fn console_spec(cwd: PathBuf, rows: u16, cols: u16, command: Option<&str>) -> SessionSpec {
    let program = match std::env::var_os(AGENT_OVERRIDE_ENV) {
        Some(over) => over,
        None => default_shell(),
    };
    let args = command
        .map(|command| shell_command_args(Path::new(&program), command))
        .unwrap_or_default();
    SessionSpec {
        program,
        args,
        cwd,
        rows,
        cols,
        env: Vec::new(),
        name: None,
        status: None,
        unreported_reason: None,
    }
}

/// Build a free-console launch through a peer environment's WSL distro.
/// `launcher` is resolved by the caller before the client upgrade; accepting it
/// here keeps argv construction pure and lets tests substitute a portable child.
/// A `command` rides after `--` as `sh -lc <command>`: the distro's login shell
/// is unknown from this side, and `sh -l` still reads the profile that puts
/// `~/.local/bin` on the PATH.
pub fn peer_console_spec(
    launcher: OsString,
    distro: &str,
    peer_path: &Path,
    (rows, cols): (u16, u16),
    command: Option<&str>,
) -> SessionSpec {
    let mut args: Vec<OsString> = vec![
        "-d".into(),
        distro.into(),
        "--cd".into(),
        peer_path.as_os_str().to_owned(),
    ];
    if let Some(command) = command {
        args.extend(["--".into(), "sh".into(), "-lc".into(), command.into()]);
    }
    SessionSpec {
        program: launcher,
        args,
        cwd: console_cwd(None),
        rows,
        cols,
        env: Vec::new(),
        name: None,
        status: None,
        unreported_reason: None,
    }
}

/// Locate the host-side WSL launcher. The agent override remains the integration
/// test seam, but unlike an agent launch it receives the real WSL argv.
pub fn peer_console_launcher() -> Option<OsString> {
    std::env::var_os(AGENT_OVERRIDE_ENV)
        .or_else(|| ralphy_proc_util::locate_program("wsl.exe").map(Into::into))
}
