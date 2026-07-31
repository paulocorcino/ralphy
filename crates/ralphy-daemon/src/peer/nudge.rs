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
//!
//! This module also answers the question that decides WHICH nudge is needed:
//! whether the distro is running at all. That belongs here rather than in
//! `client`, because this is the one seam that may invoke `wsl.exe`.

use anyhow::Result;

use super::NudgeSpec;

#[cfg(test)]
mod tests;

/// How long a nudged peer has to start answering before the caller is told it did
/// not. Generous because the act being waited on is a cold WSL boot — the distro,
/// its systemd, then the daemon — and a deadline shorter than that would report a
/// peer as failed while it was still coming up.
pub const READY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The gap between readiness probes. Each probe already carries
/// [`super::client::PEER_TIMEOUT`], so this only paces the ones that fail fast
/// (connection refused, the usual answer while the distro is still booting).
pub const READY_POLL: std::time::Duration = std::time::Duration::from_millis(500);

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

/// Whether WSL currently reports `distro` as running.
///
/// `--list --running` is a MANAGEMENT command: unlike `wsl.exe -e …` it starts
/// nothing. That is the whole reason it is usable as an observation — a probe
/// that woke what it measures could only ever answer "running", and would hand
/// the peer exactly the session whose absence was the question.
///
/// Blocking (it spawns a process), so callers on the reactor must hand it to
/// `spawn_blocking`.
#[cfg(windows)]
pub fn is_distro_running(distro: &str) -> Result<bool> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    use anyhow::Context;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let out = Command::new("wsl.exe")
        .args(["--list", "--running", "--quiet"])
        // Honoured since WSL 0.64: emit UTF-8 rather than UTF-16LE. Older builds
        // ignore it, which `decode_wsl_output` is there to survive.
        .env("WSL_UTF8", "1")
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .context("asking wsl.exe which distros are running")?;
    // The exit status is deliberately NOT checked: `--list --running` exits
    // non-zero precisely when the list is empty, which is a legitimate answer and
    // not a failure. A host that cannot answer at all fails to spawn above, and
    // that is the case worth propagating. Anything unparseable can only ever make
    // this say "not running" — the safe direction, since the nudge that follows
    // is the right act either way.
    Ok(running_contains(&decode_wsl_output(&out.stdout), distro))
}

/// `wsl.exe` is only present on a Windows host; anywhere else the question has
/// no answer, and `None` (unknown) is what the caller must fall back to.
#[cfg(not(windows))]
pub fn is_distro_running(_distro: &str) -> Result<bool> {
    anyhow::bail!("distro liveness can only be read from a Windows host")
}

/// Decode `wsl.exe` output, which is UTF-16LE on any build that does not honour
/// `WSL_UTF8`. Detected by the NUL byte every ASCII character carries in that
/// encoding — no WSL output is legitimately UTF-8 with an embedded NUL.
fn decode_wsl_output(bytes: &[u8]) -> String {
    if bytes.contains(&0) {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Whether `distro` is one of the names `output` lists, one per line.
///
/// Matching is whole-line and case-insensitive, as `wsl.exe -d` itself resolves a
/// distro name: a substring match would let `Ubuntu` claim `Ubuntu-22.04`. The
/// trim drops the BOM and the stray NUL an odd-length UTF-16 body can leave.
fn running_contains(output: &str, distro: &str) -> bool {
    output.lines().any(|line| {
        let name = line.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c == '\0');
        !name.is_empty() && name.eq_ignore_ascii_case(distro)
    })
}
