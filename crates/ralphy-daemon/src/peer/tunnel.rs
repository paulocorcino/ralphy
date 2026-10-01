//! The peer tunnel (docs/adr/0067 §2): one `ssh -N -L` local forward per peer on
//! another machine, so that peer's daemon answers on this machine's loopback and
//! the loopback gate of `peer::client` passes unchanged.
//!
//! Held the way `peer::nudge` holds a keepalive: started detached, never waited
//! on, and replaced when it has exited. It is signalled only when its host was
//! edited, because the spec it runs is then out of date. Holding a handle is not
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
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::client::{classify_self_dial, SelfRef};
use super::{PeerDescriptor, TunnelSpec};

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

/// Spawn `argv` detached: null stdin and stdout, a piped stderr (its last line
/// is why the tunnel closed, see [`watch_stderr`]), a hidden console on
/// Windows, its own process group on Unix (so a Ctrl+C to a foreground daemon
/// does not reach it). The `Child` is kept only for `try_wait`; dropping it
/// never kills.
fn spawn_detached(argv: &[String]) -> Result<Child> {
    use std::process::{Command, Stdio};

    let Some((program, rest)) = argv.split_first() else {
        anyhow::bail!("cannot spawn an empty tunnel argv");
    };
    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    ralphy_proc_util::no_window(&mut cmd);
    ralphy_proc_util::own_process_group(&mut cmd);
    cmd.spawn()
        .with_context(|| format!("starting the tunnel `{}`", argv.join(" ")))
}

/// The longest `ssh` line kept, in characters.
const SAID_MAX_CHARS: usize = 300;

/// How long a replaced `ssh`'s stderr reader may take to see EOF.
const SAID_SETTLE: Duration = Duration::from_millis(200);

/// The last non-empty line an `ssh` wrote on its stderr, kept by a thread that
/// reads the pipe to EOF.
struct Said {
    line: Arc<Mutex<Option<String>>>,
    reader: Option<JoinHandle<()>>,
}

impl Said {
    /// The last line, once the reader has seen EOF or [`SAID_SETTLE`] passed.
    fn settle(self) -> Option<String> {
        if let Some(reader) = &self.reader {
            let until = Instant::now() + SAID_SETTLE;
            while !reader.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.line.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// Take `child`'s stderr and keep its last non-empty line. The thread reads to
/// EOF, so the `ssh` never blocks on a full pipe while this daemon runs.
fn watch_stderr(child: &mut Child) -> Said {
    let line = Arc::new(Mutex::new(None));
    let reader = child.stderr.take().and_then(|stderr| {
        let slot = line.clone();
        let started = std::thread::Builder::new()
            .name("tunnel-stderr".into())
            .spawn(move || keep_last_line(stderr, &slot));
        match started {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::warn!(error = %e, "could not start the tunnel stderr reader");
                None
            }
        }
    });
    Said { line, reader }
}

fn keep_last_line(stderr: impl std::io::Read, slot: &Mutex<Option<String>>) {
    use std::io::BufRead;
    let mut stderr = std::io::BufReader::new(stderr);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match stderr.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                let text = text.trim();
                if !text.is_empty() {
                    let kept: String = text.chars().take(SAID_MAX_CHARS).collect();
                    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(kept);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                tracing::debug!(error = %e, "reading the tunnel stderr failed");
                break;
            }
        }
    }
}

/// One held `ssh`: the process, the spec it runs, and what it says.
struct Held {
    child: Child,
    spec: TunnelSpec,
    said: Said,
}

/// Behind the one [`Tunnels`] lock: the held `ssh` per peer, and the last line
/// of the one that exited before it.
#[derive(Default)]
struct State {
    held: HashMap<String, Held>,
    last_said: HashMap<String, String>,
}

/// The tunnels this process holds, one per peer `daemon_id`, each with the spec
/// it was opened with. Process-wide for the same reason as
/// `nudge::Keepalives`: the handles live as long as this daemon does, and the
/// next daemon acquires its own.
///
/// A `std::sync::Mutex`, never held across an `.await`: `ensure` spawns a
/// process, so every caller on the reactor hands it to `spawn_blocking`.
pub struct Tunnels(Mutex<State>);

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
        Tunnels(Mutex::new(State::default()))
    }

    /// Hold the tunnel to `daemon_id` open: start its `ssh` unless the one this
    /// process already started is still running with the same spec. Returns
    /// whether a new one was started. A running `ssh` whose spec differs (the
    /// host was edited) is stopped first, so the edit applies now (ADR-0067,
    /// amendment "the Hosts dialog", H4).
    pub fn ensure(&self, daemon_id: &str, spec: &TunnelSpec) -> Result<bool> {
        self.ensure_with(daemon_id, spec, || {
            let ssh = ssh_program().context("no ssh program found: install OpenSSH")?;
            spawn_detached(&tunnel_argv(&ssh, spec))
        })
    }

    /// Ensure the tunnel of every descriptor that has one, skipping this
    /// daemon's own descriptor and any tunnel whose local end is this daemon's
    /// port (the self-dial gate, logged with its usual text). A failure is
    /// logged per peer and never stops the others.
    pub fn ensure_all(&self, descriptors: &[PeerDescriptor], me: SelfRef<'_>) {
        for d in descriptors {
            if d.daemon_id == me.daemon_id {
                continue;
            }
            let Some(spec) = d.tunnel.as_ref() else {
                continue;
            };
            if let Some(refused) = classify_self_dial(&d.address, spec.local_port, me) {
                tracing::warn!(peer = %d.environment, "{}", refused.diagnosis(&d.environment));
                continue;
            }
            if let Err(e) = self.ensure(&d.daemon_id, spec) {
                tracing::warn!(peer = %d.environment, error = %format!("{e:#}"), "could not open the tunnel to a peer");
            }
        }
    }

    /// Whether the `ssh` this process holds for `daemon_id` is still running.
    pub fn is_alive(&self, daemon_id: &str) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state
            .held
            .get_mut(daemon_id)
            .is_some_and(|h| matches!(h.child.try_wait(), Ok(None)))
    }

    /// The last line the previous `ssh` to `daemon_id` wrote on its stderr
    /// before it exited, when it wrote one.
    pub fn last_said(&self, daemon_id: &str) -> Option<String> {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.last_said.get(daemon_id).cloned()
    }

    fn ensure_with(
        &self,
        daemon_id: &str,
        spec: &TunnelSpec,
        spawn: impl FnOnce() -> Result<Child>,
    ) -> Result<bool> {
        // A poisoned lock means a panic mid-insert; the map is still a map, and
        // refusing every future tunnel over it would be the worse failure.
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(held) = state.held.get_mut(daemon_id) {
            // `try_wait` is a read: it neither blocks nor signals. `Err` means
            // the handle itself is unusable, which is as good as exited.
            if matches!(held.child.try_wait(), Ok(None)) {
                if held.spec == *spec {
                    return Ok(false);
                }
                // The local port may be the same, so the old `ssh` must be gone
                // before the new one binds it: kill, then reap.
                let child = &mut held.child;
                if let Err(e) = child.kill().and_then(|()| child.wait().map(drop)) {
                    tracing::warn!(peer = %daemon_id, error = %e, "could not stop the tunnel of an edited host");
                }
            } else if let Some(exited) = state.held.remove(daemon_id) {
                // It exited by itself: what it said last is why.
                match exited.said.settle() {
                    Some(line) => state.last_said.insert(daemon_id.to_string(), line),
                    None => state.last_said.remove(daemon_id),
                };
            }
        }
        let mut child = spawn()?;
        let said = watch_stderr(&mut child);
        state.held.insert(
            daemon_id.to_string(),
            Held {
                child,
                spec: spec.clone(),
                said,
            },
        );
        Ok(true)
    }
}

/// [`Tunnels::ensure`] on the process registry, off the reactor. Also returns
/// [`Tunnels::last_said`], read after the ensure.
pub(crate) async fn hold_open(
    daemon_id: String,
    spec: TunnelSpec,
) -> Result<(bool, Option<String>)> {
    let (started, said) = tokio::task::spawn_blocking({
        let daemon_id = daemon_id.clone();
        move || {
            let started = tunnels().ensure(&daemon_id, &spec)?;
            anyhow::Ok((started, tunnels().last_said(&daemon_id)))
        }
    })
    .await
    .context("the tunnel task did not complete")??;
    if started {
        tracing::info!(peer = %daemon_id, "started the tunnel to a peer");
    }
    Ok((started, said))
}
