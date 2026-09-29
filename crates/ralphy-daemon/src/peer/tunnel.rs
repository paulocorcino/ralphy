//! The peer tunnel (docs/adr/0067 §2): one `ssh -N -L` local forward per peer on
//! another machine, so that peer's daemon answers on this machine's loopback and
//! the loopback gate of `peer::client` passes unchanged.
//!
//! Held the way `peer::nudge` holds a keepalive: started detached, never waited
//! on, never signalled, and replaced when it has exited. Holding a handle is not
//! supervision — the daemon on the other machine belongs to that machine's own
//! service manager. Unlike the keepalive, it runs on every OS.
//!
//! The ssh is the system OpenSSH, so the operator's `~/.ssh/config` (aliases,
//! jump hosts) applies with no code here. On Windows the OpenSSH that ships with
//! Windows wins over Git for Windows' `ssh`, even when Git's comes first on
//! `PATH`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result};

use super::TunnelSpec;

#[cfg(test)]
mod tests;

/// Environment override naming the program to run in place of `ssh`. A test-only
/// seam, like `RALPHY_DAEMON_AGENT_OVERRIDE`: an integration test's helper binary
/// is not reachable from a `#[cfg(test)]` path in the compiled lib.
const SSH_OVERRIDE_ENV: &str = "RALPHY_DAEMON_SSH_OVERRIDE";

/// The exact argv of the tunnel, vector form, no shell. `ServerAlive*` make a
/// forward that a VPN drop broke exit in about 45 s instead of hours; `--` keeps
/// a destination that starts with `-` from being read as an option.
pub fn tunnel_argv(ssh: &Path, spec: &TunnelSpec) -> Vec<String> {
    let mut argv = vec![
        ssh.display().to_string(),
        "-N".to_string(),
        "-L".to_string(),
        format!("127.0.0.1:{}:127.0.0.1:{}", spec.local_port, spec.peer_port),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ExitOnForwardFailure=yes".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=15".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
    ];
    if let Some(identity) = &spec.identity_file {
        argv.push("-i".to_string());
        argv.push(identity.clone());
    }
    argv.push("--".to_string());
    argv.push(spec.destination.clone());
    argv
}

/// Which `ssh` to run: `<system_root>\System32\OpenSSH\ssh.exe` when it is a
/// file, else the first `ssh` on `path_var`. Pure over its inputs; the caller
/// passes `system_root` only on Windows.
pub fn choose_ssh(
    system_root: Option<&Path>,
    path_var: Option<OsString>,
    pathext: Option<OsString>,
) -> Option<PathBuf> {
    system_root
        .map(|root| root.join("System32").join("OpenSSH").join("ssh.exe"))
        .filter(|p| p.is_file())
        .or_else(|| ralphy_proc_util::find_program("ssh", path_var, pathext))
}

/// The `ssh` this process runs, `None` when the host has none.
pub fn ssh_program() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(SSH_OVERRIDE_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let system_root = cfg!(windows)
        .then(|| std::env::var_os("SystemRoot"))
        .flatten()
        .map(PathBuf::from);
    choose_ssh(
        system_root.as_deref(),
        std::env::var_os("PATH"),
        std::env::var_os("PATHEXT"),
    )
}

/// Spawn `argv` detached: null stdio, a hidden console on Windows, its own
/// process group on Unix (so a Ctrl+C to a foreground daemon does not reach
/// it). The `Child` is kept only for `try_wait`; dropping it never kills.
fn spawn_detached(argv: &[String]) -> Result<Child> {
    use std::process::{Command, Stdio};

    let Some((program, rest)) = argv.split_first() else {
        anyhow::bail!("cannot spawn an empty tunnel argv");
    };
    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    ralphy_proc_util::no_window(&mut cmd);
    ralphy_proc_util::own_process_group(&mut cmd);
    cmd.spawn()
        .with_context(|| format!("starting the tunnel `{}`", argv.join(" ")))
}

/// The tunnels this process holds, one per peer `daemon_id`. Process-wide for
/// the same reason as `nudge::Keepalives`: the handles live as long as this
/// daemon does, and the next daemon acquires its own.
///
/// A `std::sync::Mutex`, never held across an `.await`: `ensure` spawns a
/// process, so every caller on the reactor hands it to `spawn_blocking`.
pub struct Tunnels(Mutex<HashMap<String, Child>>);

impl Default for Tunnels {
    fn default() -> Self {
        Self::new()
    }
}

static TUNNELS: LazyLock<Tunnels> = LazyLock::new(Tunnels::new);

/// This process's tunnel registry.
pub fn tunnels() -> &'static Tunnels {
    &TUNNELS
}

impl Tunnels {
    pub fn new() -> Self {
        Tunnels(Mutex::new(HashMap::new()))
    }

    /// Hold the tunnel to `daemon_id` open: start its `ssh` unless the one this
    /// process already started is still running. Returns whether a new one was
    /// started. A spec that changed while the old `ssh` runs applies only once
    /// that `ssh` exits: the holder never signals.
    pub fn ensure(&self, daemon_id: &str, spec: &TunnelSpec) -> Result<bool> {
        self.ensure_with(daemon_id, || {
            let ssh = ssh_program().context("no ssh program found: install OpenSSH")?;
            spawn_detached(&tunnel_argv(&ssh, spec))
        })
    }

    /// Whether the `ssh` this process holds for `daemon_id` is still running.
    pub fn is_alive(&self, daemon_id: &str) -> bool {
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        held.get_mut(daemon_id)
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }

    fn ensure_with(&self, daemon_id: &str, spawn: impl FnOnce() -> Result<Child>) -> Result<bool> {
        // A poisoned lock means a panic mid-insert; the map is still a map, and
        // refusing every future tunnel over it would be the worse failure.
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(child) = held.get_mut(daemon_id) {
            // `try_wait` is a read: it neither blocks nor signals. `Err` means
            // the handle itself is unusable, which is as good as exited.
            if matches!(child.try_wait(), Ok(None)) {
                return Ok(false);
            }
        }
        let child = spawn()?;
        held.insert(daemon_id.to_string(), child);
        Ok(true)
    }
}
