//! Autostart registration for the resident daemon (ADR-0032 §10): a native OS
//! mechanism that starts `ralphy daemon` at logon, without ralphy ever becoming
//! the scheduler. Windows: a shortcut in the user's Startup folder that opens
//! the `ralphyw.exe` launcher, so logon shows no window (ADR-0032, amendment
//! 2026-11-23) — no elevation, no `reg.exe`, no PowerShell. Linux/WSL: a
//! systemd user unit, `WantedBy=default.target`. macOS: a per-user launchd
//! agent under `~/Library/LaunchAgents`, loaded into the `gui/<uid>` domain
//! (ADR-0032, amendment 2026-09-15). Registration/removal is resolved with the
//! DEFAULT daemon (loopback, `DEFAULT_PORT`) — no `--bind`/`--port`
//! passthrough in v1 (ADR-0032 §4).
//!
//! Every renderer below is host-independent and takes an explicit [`Platform`]
//! (or, for Windows, plain paths) so a single host unit-tests every backend's
//! output without a live
//! scheduler (mirrors `schedule::platform`'s injected-`Platform` pattern). Only
//! the executor (`executor.rs`) is `#[cfg]`-gated to the running platform.

// This module renders every backend, but the executor is `#[cfg]`-split
// so any single-platform build calls only one of them (the others are
// exercised only from tests, which dead-code analysis ignores). The unused
// arms are live on the other OSes: expect the dead code rather than lose them.
// Every build leaves one helper unused, with or without `cfg(test)`: Windows
// and macOS never call `unit_path`, Windows never calls `run_argv`, and the
// tests call only the `pub` items. So the expectation is met on all three OSes.
#![expect(
    dead_code,
    reason = "each platform's build leaves the helpers of the other platforms unused"
)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

mod executor;
#[cfg(windows)]
mod startup;

pub use executor::{install, launchd_reload_needed, status, uninstall, AutostartStatus};

/// Which autostart backend to render for. Explicit (not `#[cfg]`) so every
/// output is testable from any host; only the executor binds this to the
/// running platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Systemd,
    Launchd,
}

/// A `systemctl --user` verb this module renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemctlVerb {
    Enable,
    Disable,
    IsEnabled,
}

/// A `launchctl` verb this module renders, all against the per-user
/// `gui/<uid>` domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchctlVerb {
    Bootstrap,
    Bootout,
    Print,
}

/// The value name of the Windows `Run` registration before 2026-11-23, the
/// systemd user unit name, and the launchd agent label. Fixed — one daemon
/// autostart registration per machine (ADR-0032 §10).
pub const TASK_NAME: &str = "ralphy-daemon";
pub const UNIT_NAME: &str = "ralphy-daemon.service";
pub const LAUNCHD_LABEL: &str = "dev.ralphy.daemon";

/// The per-user `Run` key, under HKCU, where Windows autostart was registered
/// before the Startup shortcut. `install` deletes the old value there.
pub const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Where Explorer records, by shortcut file name, a Startup entry the user
/// turned on or off in Task Manager → Startup apps.
pub const STARTUP_APPROVED_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder";

/// The file name of the daemon's shortcut in the Startup folder.
pub const SHORTCUT_NAME: &str = "Ralphy daemon.lnk";

/// The GUI-subsystem launcher the shortcut opens, beside `ralphy.exe`.
pub const LAUNCHER_NAME: &str = "ralphyw.exe";

/// The daemon's Windows Startup shortcut, as it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    /// The `.lnk` file in the Startup folder.
    pub link: PathBuf,
    /// The launcher it opens.
    pub target: PathBuf,
    /// What the launcher gets: the log file, then `daemon`.
    pub arguments: String,
    pub working_dir: PathBuf,
    pub description: String,
}

/// The shortcut that starts `ralphy daemon` through the launcher beside
/// `program`, its output appended to `log`.
pub fn windows_shortcut(program: &Path, log: &Path, startup_dir: &Path) -> Shortcut {
    let folder = program.parent().map(Path::to_path_buf).unwrap_or_default();
    Shortcut {
        link: startup_dir.join(SHORTCUT_NAME),
        target: folder.join(LAUNCHER_NAME),
        // A Windows path cannot hold a double quote, so quoting is enough.
        arguments: format!("--log \"{}\" daemon", log.display()),
        working_dir: folder,
        description: "Starts the Ralphy daemon when you sign in".to_owned(),
    }
}

/// Whether Task Manager → Startup apps has the entry turned off. Explorer
/// stores flags whose first byte is odd when the entry is off (2 on, 3 off);
/// no value means the user never touched it, which is on.
pub fn turned_off(approved: Option<&[u8]>) -> bool {
    approved
        .and_then(|bytes| bytes.first())
        .is_some_and(|flags| flags & 1 == 1)
}

/// A fully-resolved autostart registration: the program to invoke, where it
/// appends its log, and — inside WSL only — the distro name to carry into the
/// unit. Platform-neutral; it becomes a Windows shortcut, a systemd unit
/// body, or a launchd plist.
#[derive(Debug, Clone)]
pub struct AutostartSpec {
    pub program: PathBuf,
    pub log_path: PathBuf,
    /// `WSL_DISTRO_NAME` as seen by `ralphy daemon install`, which the operator
    /// runs from a login shell. See [`systemd_unit`] for why it is captured
    /// here and cannot be recovered later.
    pub wsl_distro: Option<String>,
    /// `PATH` as seen by `ralphy daemon install`. Systemd's user manager does
    /// not run the operator's shell profile, so a unit without this pin spawns
    /// `gh`/`git`/agent CLIs against the distro's stock `PATH` — where a
    /// `~/.local/bin` gh loses to an ancient `/usr/bin/gh`. See [`systemd_unit`].
    /// launchd has the same gap: a launch agent's `PATH` is the login item's,
    /// not the shell's, so [`launchd_plist`] pins it too.
    pub path: Option<String>,
    /// The owning user's uid — the launchd `gui/<uid>` domain the agent is
    /// bootstrapped into. Resolved by the executor; carried here so the
    /// `launchctl` argv renders (and unit-tests) on any host. Meaningless off
    /// macOS.
    pub uid: Option<u32>,
    /// Where the launchd plist is written (`~/Library/LaunchAgents/<label>.plist`).
    /// The `bootstrap` argv names the file, so the renderer needs it.
    pub plist_path: PathBuf,
}

/// Render the install command for `spec` on `p`.
///
/// - Systemd: the `systemctl --user enable ralphy-daemon.service` argv. The
///   unit body itself is written to disk by the executor via [`systemd_unit`],
///   not rendered as an argv.
/// - Launchd: the `launchctl bootstrap gui/<uid> <plist>` argv. The plist body
///   is written by the executor via [`launchd_plist`], like the unit.
///
/// Windows has no command: see [`windows_shortcut`].
pub fn render_install(p: Platform, spec: &AutostartSpec) -> Vec<String> {
    match p {
        Platform::Systemd => render_systemctl(SystemctlVerb::Enable),
        Platform::Launchd => render_launchctl(LaunchctlVerb::Bootstrap, spec),
    }
}

/// Render the uninstall command. Systemd: `systemctl --user disable
/// ralphy-daemon.service` (the unit FILE removal is the executor's job, not
/// rendered here). Launchd: `launchctl
/// bootout gui/<uid>/<label>` (the plist removal is likewise the executor's).
pub fn render_uninstall(p: Platform, spec: &AutostartSpec) -> Vec<String> {
    match p {
        Platform::Systemd => render_systemctl(SystemctlVerb::Disable),
        Platform::Launchd => render_launchctl(LaunchctlVerb::Bootout, spec),
    }
}

/// Render the registration-query command. Systemd: `systemctl --user
/// is-enabled ralphy-daemon.service`, whose stdout [`systemd_is_enabled`]
/// parses. Launchd: `launchctl print gui/<uid>/<label>` — exit code only;
/// `launchctl`'s text is not a contract.
pub fn render_query(p: Platform, spec: &AutostartSpec) -> Vec<String> {
    match p {
        Platform::Systemd => render_systemctl(SystemctlVerb::IsEnabled),
        Platform::Launchd => render_launchctl(LaunchctlVerb::Print, spec),
    }
}

fn render_systemctl(verb: SystemctlVerb) -> Vec<String> {
    let verb = match verb {
        SystemctlVerb::Enable => "enable",
        SystemctlVerb::Disable => "disable",
        SystemctlVerb::IsEnabled => "is-enabled",
    };
    vec![
        "systemctl".into(),
        "--user".into(),
        verb.into(),
        UNIT_NAME.into(),
    ]
}

/// The `launchctl` argv for `verb`. `bootstrap` takes the domain and the plist
/// FILE; `bootout` and `print` take the service target `gui/<uid>/<label>`.
/// A missing uid renders `gui/` with no number — `launchctl` then fails
/// loudly, which is the right outcome for a spec the executor never resolved.
fn render_launchctl(verb: LaunchctlVerb, spec: &AutostartSpec) -> Vec<String> {
    let domain = format!(
        "gui/{}",
        spec.uid.map(|u| u.to_string()).unwrap_or_default()
    );
    match verb {
        LaunchctlVerb::Bootstrap => vec![
            "launchctl".into(),
            "bootstrap".into(),
            domain,
            spec.plist_path.display().to_string(),
        ],
        LaunchctlVerb::Bootout => vec![
            "launchctl".into(),
            "bootout".into(),
            format!("{domain}/{LAUNCHD_LABEL}"),
        ],
        LaunchctlVerb::Print => vec![
            "launchctl".into(),
            "print".into(),
            format!("{domain}/{LAUNCHD_LABEL}"),
        ],
    }
}

/// The verbatim systemd user unit body for `spec` — written to
/// `~/.config/systemd/user/ralphy-daemon.service` by the executor's `install`.
///
/// When `spec.wsl_distro` is set the unit pins `WSL_DISTRO_NAME`. WSL injects
/// that variable into a **login session**, and `systemd --user` does not
/// inherit it, so a daemon started by this unit — the only start-up form
/// docs/daemon.md describes — cannot see it. Nothing inside the distro can
/// recover the name either: `/proc/sys/kernel/osrelease` proves only that this
/// *is* WSL, `/mnt/wsl` names the sibling distros rather than this one, and the
/// hostname is the Windows machine's. The install command runs from the
/// operator's shell, which is the one place the name exists, so it is captured
/// there and carried forward.
///
/// It is load-bearing rather than decorative: without it the announced peer
/// descriptor carries no [`super::peer::NudgeSpec`], so a sleeping peer cannot
/// be woken (ADR-0052 §4) and a peer free console has no distro to name.
///
/// The value is double-quoted because a distro name may contain spaces
/// (`Ubuntu 22.04` is a real default); a name carrying a quote or newline would
/// break the unit, so it is dropped rather than written malformed.
///
/// The unit also pins the installer's `PATH` (`spec.path`), for the same reason
/// with a different casualty: `systemd --user` never sources `~/.profile`, so
/// the daemon's children — `gh` for the board fold, `git`, every agent CLI —
/// resolve against the stock `PATH`. Measured live on Ubuntu-22.04: the board
/// query ran `/usr/bin/gh` 2.4 (2022, no `stateReason` field) while the
/// operator's shell had `~/.local/bin/gh` 2.89, and every board fold failed.
/// `ExecStart` is absolute and never needed the pin; the children do.
pub fn systemd_unit(spec: &AutostartSpec) -> String {
    let mut environment = String::new();
    if let Some(distro) = spec.wsl_distro.as_deref().filter(|d| unit_safe(d)) {
        writeln!(environment, "Environment=\"WSL_DISTRO_NAME={distro}\"")
            .expect(crate::WRITE_TO_STRING);
    }
    if let Some(path) = spec.path.as_deref().filter(|p| unit_safe(p)) {
        writeln!(environment, "Environment=\"PATH={path}\"").expect(crate::WRITE_TO_STRING);
    }
    format!(
        "[Unit]\nDescription=Ralphy daemon\nAfter=default.target\n\n\
         [Service]\nExecStart={} daemon\n{environment}Restart=on-failure\n\n\
         [Install]\nWantedBy=default.target\n",
        spec.program.display()
    )
}

/// Whether `value` can be written into a quoted systemd `Environment=` value
/// without changing the unit's shape. Spaces are fine; a quote, a backslash or
/// any newline is not.
fn unit_safe(value: &str) -> bool {
    !value.is_empty() && !value.contains(['"', '\\', '\n', '\r'])
}

/// Parse `systemctl --user is-enabled` stdout. `"enabled"` (the literal,
/// English-stable word `systemctl` prints — unlike `schtasks`' localized LIST
/// labels) means registered; anything else (`"disabled"`, an error, empty)
/// means not.
pub fn systemd_is_enabled(output: &str) -> bool {
    output.trim() == "enabled"
}

/// The verbatim launchd agent plist for `spec` — written to
/// `~/Library/LaunchAgents/dev.ralphy.daemon.plist` by the executor's `install`
/// and loaded with `launchctl bootstrap gui/<uid>` (ADR-0032, amendment
/// 2026-09-15). A per-user launch AGENT, not a `/Library/LaunchDaemons` daemon,
/// for the reason §10 gives on Windows: the daemon is a per-user loopback
/// resident, and an agent needs no elevation.
///
/// `RunAtLoad` starts it at login. `KeepAlive { SuccessfulExit = false }` is
/// the systemd unit's `Restart=on-failure`: launchd relaunches after a crash or
/// a signal, and leaves a clean exit alone — so `ralphy daemon restart` is not
/// fought over the port by a supervisor that resurrects what it just stopped.
///
/// `EnvironmentVariables.PATH` pins the installer's `PATH` for the reason the
/// systemd arm does (see [`systemd_unit`]): a launch agent inherits the login
/// session's minimal `PATH`, not the shell's, so `gh`/`git`/every agent CLI
/// under `~/.local/bin` or Homebrew would resolve wrong or not at all.
///
/// Both output streams append to `spec.log_path` — launchd has no journal;
/// the Windows launcher writes to the same file.
///
/// Values are XML-escaped rather than dropped when unsafe (the systemd arm's
/// `unit_safe` rule): a plist is XML, so escaping is lossless and a path
/// containing `&` round-trips instead of losing its pin.
pub fn launchd_plist(spec: &AutostartSpec) -> String {
    let program = xml_escape(&spec.program.display().to_string());
    let log = xml_escape(&spec.log_path.display().to_string());
    let environment = match spec.path.as_deref().filter(|p| !p.is_empty()) {
        Some(path) => format!(
            "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    \
             <string>{}</string>\n  </dict>\n",
            xml_escape(path)
        ),
        None => String::new(),
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20 <key>Label</key>\n\
         \x20 <string>{LAUNCHD_LABEL}</string>\n\
         \x20 <key>ProgramArguments</key>\n\
         \x20 <array>\n\
         \x20   <string>{program}</string>\n\
         \x20   <string>daemon</string>\n\
         \x20 </array>\n\
         \x20 <key>RunAtLoad</key>\n\
         \x20 <true/>\n\
         \x20 <key>KeepAlive</key>\n\
         \x20 <dict>\n\
         \x20   <key>SuccessfulExit</key>\n\
         \x20   <false/>\n\
         \x20 </dict>\n\
         {environment}\
         \x20 <key>StandardOutPath</key>\n\
         \x20 <string>{log}</string>\n\
         \x20 <key>StandardErrorPath</key>\n\
         \x20 <string>{log}</string>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// Escape the five XML-significant characters for a plist text node.
fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests;
