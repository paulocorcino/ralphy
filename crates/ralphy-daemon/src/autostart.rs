//! Autostart registration for the resident daemon (ADR-0032 §10): a native OS
//! mechanism that starts `ralphy daemon` at logon, without ralphy ever becoming
//! the scheduler. Windows: a per-user HKCU `…\CurrentVersion\Run` value,
//! launched hidden via `pwsh -WindowStyle Hidden` — no elevation. Linux/WSL: a
//! systemd user unit, `WantedBy=default.target`. macOS: a per-user launchd
//! agent under `~/Library/LaunchAgents`, loaded into the `gui/<uid>` domain
//! (ADR-0032, amendment 2026-09-15). Registration/removal is resolved with the
//! DEFAULT daemon (loopback, `DEFAULT_PORT`) — no `--bind`/`--port`
//! passthrough in v1 (ADR-0032 §4).
//!
//! Every renderer below is host-independent and takes an explicit [`Platform`]
//! so a single host unit-tests ALL THREE backends' output without a live
//! scheduler (mirrors `schedule::platform`'s injected-`Platform` pattern). Only
//! the executor at the bottom is `#[cfg]`-gated to the running platform.

// This module renders every backend, but the executor below is `#[cfg]`-split
// so any single-platform build calls only one of them (the others are
// exercised only from tests, which dead-code analysis ignores). The unused
// arms are live on the other OSes — allow rather than lose them.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;

use anyhow::{Context, Result};

/// Which autostart backend to render for. Explicit (not `#[cfg]`) so every
/// output is testable from any host; only the executor binds this to the
/// running platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
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

/// Which PowerShell the Windows Run value invokes. `pwsh` (PowerShell 7) is
/// preferred; a Windows without it still has Windows PowerShell 5.1 as
/// `powershell.exe`, and both accept the same `-NoProfile -WindowStyle Hidden
/// -Command` shape and the `*>>` all-stream redirect. Registering a command
/// whose interpreter is absent would register a daemon that never starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerShellFlavor {
    Pwsh,
    WindowsPowerShell,
}

impl PowerShellFlavor {
    fn program(self) -> &'static str {
        match self {
            PowerShellFlavor::Pwsh => "pwsh",
            PowerShellFlavor::WindowsPowerShell => "powershell",
        }
    }
}

/// Autostart registration handle (Run-key value name), the systemd user unit
/// name, and the launchd agent label. Fixed — one daemon autostart
/// registration per machine (ADR-0032 §10).
pub const TASK_NAME: &str = "ralphy-daemon";
pub const UNIT_NAME: &str = "ralphy-daemon.service";
pub const LAUNCHD_LABEL: &str = "dev.ralphy.daemon";

/// The per-user Run key Windows autostart writes to. No elevation required —
/// HKCU is writable by the owning user (ADR-0032 §10).
pub const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// A fully-resolved autostart registration: the program to invoke, where it
/// appends its log, and — inside WSL only — the distro name to carry into the
/// unit. Platform-neutral; the renderers below turn it into a `reg` argv, a
/// systemd unit body, or a launchd plist.
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
    /// Which PowerShell the Windows Run value invokes. Meaningless off Windows.
    pub shell: PowerShellFlavor,
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
/// - Windows: `reg add` writing the per-user Run key, each element one
///   argument. The `/d` value wraps the invocation in
///   `<pwsh|powershell> -NoProfile -WindowStyle Hidden -Command "…"` so the
///   daemon starts with no visible console window and the `*>>` all-stream log
///   redirect is preserved (mirrors `schedule::platform`'s log-capture fix). No
///   elevation: HKCU is writable by the owning user. The interpreter is
///   `spec.shell` — see [`PowerShellFlavor`].
/// - Systemd: the `systemctl --user enable ralphy-daemon.service` argv. The
///   unit body itself is written to disk by the executor via [`systemd_unit`],
///   not rendered as an argv.
/// - Launchd: the `launchctl bootstrap gui/<uid> <plist>` argv. The plist body
///   is written by the executor via [`launchd_plist`], like the unit.
pub fn render_install(p: Platform, spec: &AutostartSpec) -> Vec<String> {
    match p {
        Platform::Windows => {
            let exe = spec.program.display();
            let log = spec.log_path.display();
            let shell = spec.shell.program();
            // Single-quote the paths so PowerShell tolerates spaces; the outer
            // double-quotes belong to the `-Command` argument, not to `reg`
            // shell-quoting (we pass this argv straight to CreateProcess).
            let tr = format!(
                "{shell} -NoProfile -WindowStyle Hidden -Command \"'{exe}' daemon *>> '{log}'\""
            );
            vec![
                "reg".into(),
                "add".into(),
                RUN_KEY.into(),
                "/v".into(),
                TASK_NAME.into(),
                "/t".into(),
                "REG_SZ".into(),
                "/d".into(),
                tr,
                "/f".into(),
            ]
        }
        Platform::Systemd => render_systemctl(SystemctlVerb::Enable),
        Platform::Launchd => render_launchctl(LaunchctlVerb::Bootstrap, spec),
    }
}

/// Render the uninstall command. Windows: `reg delete <RUN_KEY> /v <task> /f`.
/// Systemd: `systemctl --user disable ralphy-daemon.service` (the unit FILE
/// removal is the executor's job, not rendered here). Launchd: `launchctl
/// bootout gui/<uid>/<label>` (the plist removal is likewise the executor's).
pub fn render_uninstall(p: Platform, spec: &AutostartSpec) -> Vec<String> {
    match p {
        Platform::Windows => vec![
            "reg".into(),
            "delete".into(),
            RUN_KEY.into(),
            "/v".into(),
            TASK_NAME.into(),
            "/f".into(),
        ],
        Platform::Systemd => render_systemctl(SystemctlVerb::Disable),
        Platform::Launchd => render_launchctl(LaunchctlVerb::Bootout, spec),
    }
}

/// Render the registration-query command. Windows: `reg query <RUN_KEY> /v
/// <task>` — the executor reads only its EXIT CODE (0 when the value exists,
/// 1 when absent; never localized text — see the module-level pt-BR trap this
/// sidesteps). Systemd: `systemctl --user is-enabled ralphy-daemon.service`,
/// whose stdout [`systemd_is_enabled`] parses. Launchd: `launchctl print
/// gui/<uid>/<label>` — exit code only, like `reg query`; `launchctl`'s text is
/// not a contract.
pub fn render_query(p: Platform, spec: &AutostartSpec) -> Vec<String> {
    match p {
        Platform::Windows => vec![
            "reg".into(),
            "query".into(),
            RUN_KEY.into(),
            "/v".into(),
            TASK_NAME.into(),
        ],
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
        environment.push_str(&format!("Environment=\"WSL_DISTRO_NAME={distro}\"\n"));
    }
    if let Some(path) = spec.path.as_deref().filter(|p| unit_safe(p)) {
        environment.push_str(&format!("Environment=\"PATH={path}\"\n"));
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
/// the Windows Run value uses the same file.
///
/// Values are XML-escaped rather than dropped when unsafe (the systemd arm's
/// [`unit_safe`] rule): a plist is XML, so escaping is lossless and a path
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

// --- host executor (the only `#[cfg]`-gated seam) --------------------------

/// Whether the daemon's autostart registration currently exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutostartStatus {
    pub registered: bool,
}

fn home_dir() -> Result<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .context("could not resolve a home directory for daemon autostart")?;
    Ok(PathBuf::from(home))
}

/// `<home>/.ralphy/daemon.log` — where the Windows task appends daemon
/// output (the systemd path relies on the journal instead).
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
    let exe = std::env::current_exe().context("locating the running ralphy binary")?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

/// Which PowerShell to register on Windows: `pwsh` when some `PATH` entry holds
/// `pwsh.exe`, else Windows PowerShell, which every supported Windows ships.
/// A `PATH` walk rather than a probe spawn — this is resolved at install time
/// and spawning is the expensive thing on Windows.
pub fn powershell_flavor(path: Option<&str>) -> PowerShellFlavor {
    let found = path
        .into_iter()
        .flat_map(std::env::split_paths)
        .any(|dir| dir.join("pwsh.exe").is_file());
    if found {
        PowerShellFlavor::Pwsh
    } else {
        PowerShellFlavor::WindowsPowerShell
    }
}

/// The uid that owns this process — the launchd `gui/<uid>` domain.
#[cfg(unix)]
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
        shell: powershell_flavor(path.as_deref()),
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
    let status = ProcCommand::new(prog)
        .args(rest)
        .status()
        .with_context(|| format!("running {prog}"))?;
    if !status.success() && !tolerate_missing {
        anyhow::bail!("{prog} exited with {status}");
    }
    Ok(())
}

/// Run `argv` and report only whether it exited `0` — the shape the Windows
/// and launchd status probes use, because neither tool's text is a contract.
fn argv_succeeds(argv: &[String]) -> Result<bool> {
    let (prog, rest) = argv.split_first().context("empty autostart command")?;
    let out = ProcCommand::new(prog)
        .args(rest)
        .output()
        .with_context(|| format!("running {prog}"))?;
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

#[cfg(windows)]
pub fn install() -> Result<()> {
    let spec = build_spec()?;
    run_argv(&render_install(Platform::Windows, &spec), false).with_context(|| {
        format!(
            "could not register daemon autostart (writing {RUN_KEY}\\{TASK_NAME}); \
             this should not require elevation — check that the registry is writable"
        )
    })
}

#[cfg(windows)]
pub fn uninstall() -> Result<()> {
    // `/F` on install already makes registration idempotent; tolerate a
    // "task not found" exit here so a second uninstall is a clean no-op.
    run_argv(&render_uninstall(Platform::Windows, &build_spec()?), true)
}

#[cfg(windows)]
pub fn status() -> Result<AutostartStatus> {
    // `reg query` EXIT CODE only (0 = present, 1 = absent) — sidesteps the
    // pt-BR localized label bug (KNOWLEDGE #139/#140) that misreported a real
    // `schtasks` task as absent; `reg query` has no such localized text path.
    Ok(AutostartStatus {
        registered: argv_succeeds(&render_query(Platform::Windows, &build_spec()?))?,
    })
}

#[cfg(target_os = "macos")]
pub fn install() -> Result<()> {
    let spec = build_spec()?;
    write_registration(&spec.plist_path, &launchd_plist(&spec))?;
    // launchd opens `StandardOutPath` itself and does not create its parent;
    // on a fresh account `~/.ralphy` may not exist yet and the agent would
    // fail to spawn with nothing logged anywhere.
    if let Some(parent) = spec.log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // `bootstrap` refuses a label that is already loaded, so a re-install
    // would fail where the Run value (`/f`) and `systemctl enable` are
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
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn install() -> Result<()> {
    let spec = build_spec()?;
    write_registration(&unit_path()?, &systemd_unit(&spec))?;
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
    let out = ProcCommand::new(prog)
        .args(rest)
        .output()
        .with_context(|| format!("running {prog}"))?;
    Ok(AutostartStatus {
        registered: systemd_is_enabled(&String::from_utf8_lossy(&out.stdout)),
    })
}

#[cfg(test)]
mod tests;
