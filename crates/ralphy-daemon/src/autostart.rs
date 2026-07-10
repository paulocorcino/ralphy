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

/// A fully-resolved autostart registration: the program to invoke and where it
/// appends its log. Platform-neutral; the renderers below turn it into a
/// `schtasks` argv or a systemd unit body.
#[derive(Debug, Clone)]
pub struct AutostartSpec {
    pub program: PathBuf,
    pub log_path: PathBuf,
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
pub fn systemd_unit(spec: &AutostartSpec) -> String {
    format!(
        "[Unit]\nDescription=Ralphy daemon\nAfter=default.target\n\n\
         [Service]\nExecStart={} daemon\nRestart=on-failure\n\n\
         [Install]\nWantedBy=default.target\n",
        spec.program.display()
    )
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
mod tests {
    use super::*;

    fn spec() -> AutostartSpec {
        AutostartSpec {
            program: PathBuf::from("/usr/local/bin/ralphy"),
            log_path: PathBuf::from("/home/me/.ralphy/daemon.log"),
        }
    }

    #[test]
    fn render_install_windows_runkey() {
        let joined = render_install(Platform::Windows, &spec()).join(" ");
        for needle in [
            "reg",
            "add",
            RUN_KEY,
            "/v",
            TASK_NAME,
            "REG_SZ",
            "-WindowStyle Hidden",
            "daemon",
            "*>>",
        ] {
            assert!(joined.contains(needle), "missing {needle:?} in {joined:?}");
        }
        assert!(!joined.contains("schtasks"), "{joined:?}");
        assert!(!joined.contains("ONLOGON"), "{joined:?}");
    }

    #[test]
    fn render_uninstall_windows() {
        let joined = render_uninstall(Platform::Windows).join(" ");
        for needle in ["reg", "delete", RUN_KEY, "/v", TASK_NAME, "/f"] {
            assert!(joined.contains(needle), "missing {needle:?} in {joined:?}");
        }
    }

    #[test]
    fn render_query_windows() {
        let joined = render_query(Platform::Windows).join(" ");
        for needle in ["reg", "query", RUN_KEY, "/v", TASK_NAME] {
            assert!(joined.contains(needle), "missing {needle:?} in {joined:?}");
        }
    }

    #[test]
    fn uninstall_targets_the_installed_task() {
        let install_joined = render_install(Platform::Windows, &spec()).join(" ");
        let uninstall_joined = render_uninstall(Platform::Windows).join(" ");
        assert!(install_joined.contains(TASK_NAME));
        assert!(install_joined.contains(RUN_KEY));
        assert!(uninstall_joined.contains("delete"));
        assert!(uninstall_joined.contains(TASK_NAME));
        assert!(uninstall_joined.contains(RUN_KEY));

        let disable = render_uninstall(Platform::Systemd).join(" ");
        assert!(disable.contains("disable"), "{disable:?}");
        assert!(disable.contains(UNIT_NAME), "{disable:?}");
    }

    #[test]
    fn systemd_unit_has_execstart_and_wantedby() {
        let unit = systemd_unit(&spec());
        for needle in [
            "[Unit]",
            "[Service]",
            "[Install]",
            "Description=Ralphy daemon",
            "ExecStart=",
            "daemon",
            "WantedBy=default.target",
        ] {
            assert!(unit.contains(needle), "missing {needle:?} in {unit:?}");
        }
    }

    #[test]
    fn render_enable_disable_systemd() {
        let enable = render_install(Platform::Systemd, &spec()).join(" ");
        let disable = render_uninstall(Platform::Systemd).join(" ");
        for joined in [&enable, &disable] {
            assert!(joined.contains("systemctl"), "{joined:?}");
            assert!(joined.contains("--user"), "{joined:?}");
            assert!(joined.contains(UNIT_NAME), "{joined:?}");
        }
        assert!(enable.contains("enable"), "{enable:?}");
        assert!(disable.contains("disable"), "{disable:?}");
    }

    #[test]
    fn systemd_is_enabled_parses_literal() {
        assert!(systemd_is_enabled("enabled\n"));
        assert!(!systemd_is_enabled("disabled\n"));
        assert!(!systemd_is_enabled(""));
    }
}
