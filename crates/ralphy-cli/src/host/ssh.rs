//! The SSH seam: every host command goes through [`HostShell`], so tests replace
//! the real `ssh` with a fake and no automatic test signs in anywhere.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// What one host command returned. `code` is `None` when `ssh` was killed by a
/// signal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct HostOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl HostOutput {
    pub(crate) fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

pub(crate) trait HostShell {
    /// Run `command` on the host, feeding it `stdin`. `identity` is a key file
    /// to sign in with instead of the operator's SSH config or agent.
    fn run(&mut self, identity: Option<&Path>, command: &str, stdin: &[u8]) -> Result<HostOutput>;
}

/// The `ssh` argv for one command. `BatchMode=yes` means ssh never prompts, so
/// a sign-in that works here also works for the tunnel with no person present;
/// `StrictHostKeyChecking=yes` refuses a host whose key is not known yet.
pub(crate) fn ssh_argv(
    ssh: &Path,
    destination: &str,
    identity: Option<&Path>,
    command: &str,
) -> Vec<String> {
    let mut argv = vec![ssh.display().to_string()];
    for opt in [
        "BatchMode=yes",
        "StrictHostKeyChecking=yes",
        "ConnectTimeout=15",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
    ] {
        argv.push("-o".to_string());
        argv.push(opt.to_string());
    }
    if let Some(id) = identity {
        argv.push("-i".to_string());
        argv.push(id.display().to_string());
        argv.push("-o".to_string());
        argv.push("IdentitiesOnly=yes".to_string());
    }
    argv.push("--".to_string());
    argv.push(destination.to_string());
    argv.push(command.to_string());
    argv
}

/// The system `ssh`.
pub(crate) struct Ssh {
    program: PathBuf,
    destination: String,
}

impl Ssh {
    pub(crate) fn new(destination: &str) -> Result<Ssh> {
        let program = ralphy_daemon::peer::tunnel::ssh_program()
            .context("no ssh program found: install OpenSSH")?;
        Ok(Ssh {
            program,
            destination: destination.to_string(),
        })
    }

    pub(crate) fn program(&self) -> &Path {
        &self.program
    }
}

impl HostShell for Ssh {
    fn run(&mut self, identity: Option<&Path>, command: &str, stdin: &[u8]) -> Result<HostOutput> {
        let argv = ssh_argv(&self.program, &self.destination, identity, command);
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        ralphy_proc_util::no_window(&mut cmd);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {}", self.program.display()))?;
        let mut input = child.stdin.take().context("ssh has no stdin pipe")?;
        let bytes = stdin.to_vec();
        // A separate writer, so a host that answers before it reads all of its
        // input cannot fill the output pipe and block both sides.
        let writer = std::thread::spawn(move || input.write_all(&bytes));
        let out = child
            .wait_with_output()
            .with_context(|| format!("waiting for ssh {}", self.destination))?;
        match writer.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            Ok(Err(e)) => return Err(e).context("sending input to the host"),
            Err(_) => anyhow::bail!("the thread sending input to the host panicked"),
        }
        Ok(HostOutput {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Why `ssh` itself failed, as opposed to the remote command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SshFailure {
    HostKeyUnknown,
    HostKeyChanged,
    AuthRefused,
    Unreachable,
    Other,
}

impl SshFailure {
    /// The `kind` of a `failed` progress event.
    pub(crate) fn key(self) -> &'static str {
        match self {
            SshFailure::HostKeyUnknown => "host_key_unknown",
            SshFailure::HostKeyChanged => "host_key_changed",
            SshFailure::AuthRefused => "auth_refused",
            SshFailure::Unreachable => "unreachable",
            SshFailure::Other => "other",
        }
    }
}

/// A classified connection failure, so a caller can tell an unreachable host
/// from a refused key without reading the message.
#[derive(Debug)]
pub(crate) struct SshError {
    pub kind: SshFailure,
    pub message: String,
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SshError {}

pub(crate) fn ssh_error(kind: SshFailure, message: String) -> anyhow::Error {
    SshError { kind, message }.into()
}

/// `ssh` exits 255 on its own errors; any other code is the remote command's,
/// so its stderr says nothing about the connection.
pub(crate) fn classify(out: &HostOutput) -> Option<SshFailure> {
    if out.code != Some(255) {
        return None;
    }
    let err = &out.stderr;
    let has = |needle: &str| err.contains(needle);
    Some(if has("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        SshFailure::HostKeyChanged
    } else if has("Host key verification failed") || has("host key is known") {
        SshFailure::HostKeyUnknown
    } else if has("Permission denied") {
        SshFailure::AuthRefused
    } else if has("Connection refused")
        || has("timed out")
        || has("Could not resolve hostname")
        || has("No route to host")
    {
        SshFailure::Unreachable
    } else {
        SshFailure::Other
    })
}

#[cfg(test)]
pub(crate) mod tests;
