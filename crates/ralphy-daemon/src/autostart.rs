//! Autostart registration for the resident daemon (ADR-0032 §10): a native OS
//! mechanism that starts `ralphy daemon` at logon, without ralphy ever becoming
//! the scheduler. Windows: a per-user HKCU `…\CurrentVersion\Run` value,
//! launched hidden via `pwsh -WindowStyle Hidden` — no elevation. Linux/WSL: a
//! systemd user unit, `WantedBy=default.target`. Registration/removal is
//! resolved with the DEFAULT daemon (loopback, `DEFAULT_PORT`) — no
//! `--bind`/`--port` passthrough in v1 (ADR-0032 §4).
//!
//! Every renderer below is host-independent and takes an explicit [`Platform`]
//! so a single host unit-tests BOTH backends' output without a live scheduler
//! (mirrors `schedule::platform`'s injected-`Platform` pattern). Only the
//! executor at the bottom is `#[cfg]`-gated to the running platform.

// This module renders BOTH backends, but the executor below is `#[cfg]`-split
// so any single-platform build calls only half of it (the other half is
// exercised only from tests, which dead-code analysis ignores). The unused
// half is live on the other OS — allow rather than lose it.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command as ProcCommand;

use anyhow::{Context, Result};

/// Which autostart backend to render for. Explicit (not `#[cfg]`) so both
/// outputs are testable from either host; only the executor binds this to the
/// running platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Systemd,
}

/// A `systemctl --user` verb this module renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemctlVerb {
    Enable,
    Disable,
    IsEnabled,
}

/// Autostart registration handle (Run-key value name), and the systemd user
/// unit name. Fixed — one daemon autostart registration per machine
/// (ADR-0032 §10).
pub const TASK_NAME: &str = "ralphy-daemon";
pub const UNIT_NAME: &str = "ralphy-daemon.service";

/// The per-user Run key Windows autostart writes to. No elevation required —
/// HKCU is writable by the owning user (ADR-0032 §10).
pub const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// A fully-resolved autostart registration: the program to invoke, where it
/// appends its log, and — inside WSL only — the distro name to carry into the
/// unit. Platform-neutral; the renderers below turn it into a `reg` argv or a
/// systemd unit body.
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
    pub path: Option<String>,
}

/// Render the install command for `spec` on `p`.
///
/// - Windows: `reg add` writing the per-user Run key, each element one
///   argument. The `/d` value wraps the invocation in
///   `pwsh -NoProfile -WindowStyle Hidden -Command "…"` so the daemon starts
///   with no visible console window and the `*>>` all-stream log redirect is
///   preserved (mirrors `schedule::platform`'s log-capture fix). No elevation:
///   HKCU is writable by the owning user.
/// - Systemd: the `systemctl --user enable ralphy-daemon.service` argv. The
///   unit body itself is written to disk by the executor via [`systemd_unit`],
///   not rendered as an argv.
pub fn render_install(p: Platform, spec: &AutostartSpec) -> Vec<String> {
    match p {
        Platform::Windows => {
            let exe = spec.program.display();
            let log = spec.log_path.display();
            // Single-quote the paths so PowerShell tolerates spaces; the outer
            // double-quotes belong to the `-Command` argument, not to `reg`
            // shell-quoting (we pass this argv straight to CreateProcess).
            let tr = format!(
                "pwsh -NoProfile -WindowStyle Hidden -Command \"'{exe}' daemon *>> '{log}'\""
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
    }
}

/// Render the uninstall command. Windows: `reg delete <RUN_KEY> /v <task> /f`.
/// Systemd: `systemctl --user disable ralphy-daemon.service` (the unit FILE
/// removal is the executor's job, not rendered here).
pub fn render_uninstall(p: Platform) -> Vec<String> {
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
    }
}

/// Render the registration-query command. Windows: `reg query <RUN_KEY> /v
/// <task>` — the executor reads only its EXIT CODE (0 when the value exists,
/// 1 when absent; never localized text — see the module-level pt-BR trap this
/// sidesteps). Systemd: `systemctl --user is-enabled ralphy-daemon.service`,
/// whose stdout [`systemd_is_enabled`] parses.
pub fn render_query(p: Platform) -> Vec<String> {
    match p {
        Platform::Windows => vec![
            "reg".into(),
            "query".into(),
            RUN_KEY.into(),
            "/v".into(),
            TASK_NAME.into(),
        ],
        Platform::Systemd => render_systemctl(SystemctlVerb::IsEnabled),
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

/// The canonicalized absolute path to the running binary, so the registered
/// autostart resolves `ralphy` regardless of the scheduler's stripped PATH.
/// Mirrors `schedule.rs::current_exe`.
fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating the running ralphy binary")?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

fn build_spec() -> Result<AutostartSpec> {
    Ok(AutostartSpec {
        program: current_exe()?,
        log_path: daemon_log_path()?,
        // Read here, at install time, from the operator's own shell — the only
        // context that has it. See [`systemd_unit`].
        wsl_distro: std::env::var("WSL_DISTRO_NAME")
            .ok()
            .filter(|d| !d.is_empty()),
        // Same reasoning: the operator's shell is the one place their PATH —
        // `~/.local/bin`, `~/.cargo/bin`, the vendor CLIs — is assembled.
        path: std::env::var("PATH").ok().filter(|p| !p.is_empty()),
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
    run_argv(&render_uninstall(Platform::Windows), true)
}

#[cfg(windows)]
pub fn status() -> Result<AutostartStatus> {
    let argv = render_query(Platform::Windows);
    let (prog, rest) = argv.split_first().context("empty autostart command")?;
    let out = ProcCommand::new(prog)
        .args(rest)
        .output()
        .with_context(|| format!("running {prog}"))?;
    // `reg query` EXIT CODE only (0 = present, 1 = absent) — sidesteps the
    // pt-BR localized label bug (KNOWLEDGE #139/#140) that misreported a real
    // `schtasks` task as absent; `reg query` has no such localized text path.
    Ok(AutostartStatus {
        registered: out.status.success(),
    })
}

#[cfg(not(windows))]
pub fn install() -> Result<()> {
    let spec = build_spec()?;
    let path = unit_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, systemd_unit(&spec))
        .with_context(|| format!("writing {}", path.display()))?;
    run_argv(&render_install(Platform::Systemd, &spec), false)
}

#[cfg(not(windows))]
pub fn uninstall() -> Result<()> {
    // Tolerate "unit not found" so a second uninstall is a clean no-op.
    run_argv(&render_uninstall(Platform::Systemd), true)?;
    let path = unit_path()?;
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn status() -> Result<AutostartStatus> {
    let argv = render_query(Platform::Systemd);
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
