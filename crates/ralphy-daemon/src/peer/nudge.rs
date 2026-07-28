//! Waking a peer that is not answering (docs/adr/0052 §4): the nudge.
//!
//! The nudging daemon does NOT supervise what it starts — it spawns `wsl.exe`
//! detached and forgets it. Ralphy's launcher model is "start, never parent"
//! (ADR-0032), and a systemd user unit is already supervised by systemd; a
//! second parent would only add a zombie to reap.
//!
//! The argv builder is pure and platform-neutral so both CI legs prove the shape
//! — a nudge must never be assembled as a shell string, which is what keeps a
//! distro or unit name from ever being interpreted.

use anyhow::Result;

use super::NudgeSpec;

#[cfg(test)]
mod tests;

/// The exact argv that starts `spec.unit` inside `spec.distro`. Vector form, no
/// shell: `wsl.exe -d <distro> -e systemctl --user start <unit>`.
pub fn nudge_argv(spec: &NudgeSpec) -> Vec<String> {
    vec![
        "wsl.exe".to_string(),
        "-d".to_string(),
        spec.distro.clone(),
        "-e".to_string(),
        "systemctl".to_string(),
        "--user".to_string(),
        "start".to_string(),
        spec.unit.clone(),
    ]
}

/// Spawn `argv` detached: no console window, null stdio, and the `Child` is
/// DROPPED without `wait` — the nudger never parents, holds, or signals what it
/// started (ADR-0052 §4).
#[cfg(windows)]
pub fn spawn_detached(argv: &[String]) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    use anyhow::{bail, Context};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    let Some((program, rest)) = argv.split_first() else {
        bail!("cannot spawn an empty nudge argv");
    };
    Command::new(program)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .with_context(|| format!("spawning the nudge `{}`", argv.join(" ")))?;
    // Child dropped here on purpose: see the module doc.
    Ok(())
}

/// `wsl.exe` exists only on a Windows host, so a nudge from anywhere else is a
/// refusal, not a spawn.
#[cfg(not(windows))]
pub fn spawn_detached(_argv: &[String]) -> Result<()> {
    anyhow::bail!("nudge is only available from a Windows host")
}
