//! Waking a peer that is not answering (docs/adr/0052 §4): the nudge, and the
//! keepalive that makes the wake last.
//!
//! The nudging daemon does NOT supervise what it starts — it spawns `wsl.exe`
//! detached and forgets it. Ralphy's launcher model is "start, never parent"
//! (ADR-0032), and a systemd user unit is already supervised by systemd; a
//! second parent would only add a zombie to reap.
//!
//! A wake alone does not last. WSL keeps a distro running only while some
//! Windows-side `wsl.exe` holds a session in it; a `systemd --user` unit does
//! not count, lingering or not, so `vmIdleTimeout` (30 s on the reference host)
//! ends the distro — and the peer, and every PTY session it owns — about 20 s
//! after the nudge's own `wsl.exe` exits (measured 2026-09-21, three runs).
//! The keepalive is that handle: one idle `wsl.exe -d <distro> -e sleep
//! infinity` per distro, detached, never waited on, never signalled. Holding a
//! handle is not supervising: the daemon inside is still systemd's, and the
//! handle outlives the daemon that opened it (ADR-0052 §4 amendment).
//!
//! The argv builders are pure and platform-neutral so both CI legs prove the
//! shape — a nudge must never be assembled as a shell string, which is what
//! keeps a distro or unit name from ever being interpreted.
//!
//! This module also answers the question that decides WHICH nudge is needed:
//! whether the distro is running at all. That belongs here rather than in
//! `client`, because this is the one seam that may invoke `wsl.exe`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{LazyLock, Mutex};

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

/// The exact argv of the keepalive that holds `spec.distro` open. Vector form,
/// no shell: `wsl.exe -d <distro> -e sleep infinity`. `sleep` is coreutils, so
/// the distro needs nothing installed; `-e` skips the login shell, so no rc file
/// runs for it.
pub fn keepalive_argv(spec: &NudgeSpec) -> Vec<String> {
    vec![
        "wsl.exe".to_string(),
        "-d".to_string(),
        spec.distro.clone(),
        "-e".to_string(),
        "sleep".to_string(),
        "infinity".to_string(),
    ]
}

/// Spawn `argv` detached: a hidden console, null stdio, and the `Child` is
/// DROPPED without `wait` — the nudger never parents, holds, or signals what it
/// started (ADR-0052 §4).
pub fn spawn_detached(argv: &[String]) -> Result<()> {
    spawn_detached_child(argv).map(drop)
}

/// The spawn behind [`spawn_detached`], handing back the `Child` for a caller
/// that needs to ask later whether it is still running. Dropping a `Child` never
/// kills it, so keeping the handle changes nothing about the process.
///
/// A HIDDEN console, never `DETACHED_PROCESS`: `wsl.exe` re-executes itself
/// (measured 2026-09-22, WSL from the Store: `wsl.exe` → `wsl.exe`), and a
/// console child of a process with no console gets a fresh VISIBLE one. Windows
/// 11 hands that to Windows Terminal, so every keepalive opened an empty
/// `C:\Program Files\WSL\wsl.exe` tab. The hidden console is the child's own,
/// not the daemon's, so the daemon's console events still never reach it.
#[cfg(windows)]
fn spawn_detached_child(argv: &[String]) -> Result<Child> {
    use std::process::{Command, Stdio};

    use anyhow::{bail, Context};

    let Some((program, rest)) = argv.split_first() else {
        bail!("cannot spawn an empty nudge argv");
    };
    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    ralphy_proc_util::no_window(&mut cmd);
    cmd.spawn()
        .with_context(|| format!("spawning the nudge `{}`", argv.join(" ")))
}

/// `wsl.exe` exists only on a Windows host, so a nudge from anywhere else is a
/// refusal, not a spawn.
#[cfg(not(windows))]
fn spawn_detached_child(_argv: &[String]) -> Result<Child> {
    anyhow::bail!("nudge is only available from a Windows host")
}

/// The keepalives this process holds, one per distro. Process-wide because that
/// is what it models: the handles live as long as this daemon does — a
/// keepalive is never owned by a router or a request.
///
/// A keepalive outlives the daemon that spawned it, so its pid is also written
/// to the store (`keepalive-<distro>.pid`). The next daemon adopts a keepalive
/// that is still running instead of starting a second one. Every daemon exit
/// that is not a tree kill (a job ended, an update hand-over, a crash) leaves
/// the keepalive running; with no adoption, each next daemon adds one more
/// (measured 2026-10-01: nine on one host).
///
/// A `std::sync::Mutex`, never held across an `.await`: `ensure` spawns a
/// process, so every caller on the reactor hands it to `spawn_blocking`.
pub struct Keepalives(Mutex<HashMap<String, Held>>);

/// One distro's keepalive: the child this process spawned, or the pid of one a
/// previous daemon spawned. An adopted pid is only ever read, never signalled.
enum Held {
    Spawned(Child),
    Adopted(u32),
}

static KEEPALIVES: LazyLock<Keepalives> = LazyLock::new(|| Keepalives(Mutex::new(HashMap::new())));

/// This process's keepalive registry.
pub fn keepalives() -> &'static Keepalives {
    &KEEPALIVES
}

impl Keepalives {
    /// Hold `spec.distro` open: spawn its keepalive unless one is still
    /// running — the one this process spawned, or the one the store's pid file
    /// names. Returns whether a new one was spawned.
    ///
    /// Idempotent on purpose — a nudge per chip click, plus one at every daemon
    /// start, must add up to one `wsl.exe` per distro, not one per event. A
    /// keepalive that exited (the distro was shut down, `wsl --shutdown`, the
    /// name no longer resolves) is replaced, since a dead handle holds nothing.
    pub fn ensure(&self, spec: &NudgeSpec) -> Result<bool> {
        let store = crate::auth::store_dir()?;
        self.ensure_with(&spec.distro, &store, is_running_keepalive, || {
            spawn_detached_child(&keepalive_argv(spec))
        })
    }

    /// `is_keepalive` answers whether a pid is a running keepalive; injected so
    /// a test can name a pid without a real `wsl.exe`.
    fn ensure_with(
        &self,
        distro: &str,
        store: &Path,
        is_keepalive: impl Fn(u32) -> bool,
        spawn: impl FnOnce() -> Result<Child>,
    ) -> Result<bool> {
        // A poisoned lock means a panic mid-insert; the map is still a map, and
        // refusing every future wake over it would be the worse failure.
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let still_running = match held.get_mut(distro) {
            // `try_wait` is the one question asked of the child, and it is a
            // read: it neither blocks nor signals. `Err` means the handle itself
            // is unusable, which is as good as exited.
            Some(Held::Spawned(child)) => matches!(child.try_wait(), Ok(None)),
            Some(Held::Adopted(pid)) => is_keepalive(*pid),
            None => false,
        };
        if still_running {
            return Ok(false);
        }
        let pid_file = keepalive_pid_file(store, distro);
        if let Some(pid) = read_pid(&pid_file).filter(|pid| is_keepalive(*pid)) {
            tracing::info!(%distro, pid, "keeping the WSL distro running with the keepalive a previous daemon started");
            held.insert(distro.to_string(), Held::Adopted(pid));
            return Ok(false);
        }
        let child = spawn()?;
        // The keepalive already holds the distro; a pid file that cannot be
        // written only costs the next daemon one more `wsl.exe`.
        if let Err(e) = std::fs::write(&pid_file, child.id().to_string()) {
            tracing::warn!(%distro, path = %pid_file.display(), error = %e, "could not record the keepalive pid");
        }
        held.insert(distro.to_string(), Held::Spawned(child));
        Ok(true)
    }
}

/// `<store>/keepalive-<distro>.pid`. A distro name is free text to WSL, so
/// anything outside `[A-Za-z0-9._-]` becomes `_` before it is a file name.
fn keepalive_pid_file(store: &Path, distro: &str) -> PathBuf {
    let safe: String = distro
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    store.join(format!("keepalive-{safe}.pid"))
}

/// The pid a keepalive file holds; `None` for a missing or unreadable file,
/// which is the same answer as "no keepalive to adopt".
fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// A pid is a keepalive to adopt when it is alive and runs `wsl.exe`. The
/// image check guards against a reused pid; a reused pid that is some other
/// `wsl.exe` would only keep this distro's handle out of the registry until it
/// exits, and the next ensure replaces it.
fn is_running_keepalive(pid: u32) -> bool {
    ralphy_proc_util::pid_is_alive(pid)
        && ralphy_proc_util::pid::exe_of_pid(pid)
            .and_then(|exe| exe.file_name().map(|n| n.eq_ignore_ascii_case("wsl.exe")))
            .unwrap_or(false)
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
#[cfg(any(windows, test))]
fn decode_wsl_output(bytes: &[u8]) -> String {
    if bytes.contains(&0) {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
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
#[cfg(any(windows, test))]
fn running_contains(output: &str, distro: &str) -> bool {
    output.lines().any(|line| {
        let name = line.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c == '\0');
        !name.is_empty() && name.eq_ignore_ascii_case(distro)
    })
}
