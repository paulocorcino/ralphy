//! Signing in to a host: the operator's SSH config or agent, then the peer key,
//! then the operator's password, which adds the peer key on the host.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use ralphy_daemon::peer::key::ensure_peer_key;

use super::super::password::add_peer_key;
use super::super::report::Report;
use super::super::shell::{keys_path, render, HostOp, HostOs};
use super::super::ssh::{classify, ssh_error, HostOutput, HostShell, SshError, SshFailure};

pub(super) fn unknown_host(dest: &str) -> String {
    format!(
        "the host key of {dest} is not known yet, so Ralphy sent nothing to it. Run `ssh {dest}` once, \
         check that the fingerprint it shows is the host's, answer yes, then run this command again"
    )
}

pub(super) fn changed_host(dest: &str) -> String {
    format!(
        "the host key of {dest} has changed, so Ralphy sent nothing to it. Find out why the key changed \
         before you trust it: `ssh {dest}` shows the details"
    )
}

fn both_refused(dest: &str, public_line: &str) -> String {
    format!(
        "the host {dest} refused your SSH key and Ralphy's key. Give the account password \
         (the workbench asks for it; the command reads it with --password-stdin), and Ralphy adds its key \
         there. Or add this line to .ssh/authorized_keys on the host \
         (for a Windows administrator: {}), then run this command again:\n{public_line}\n\
         A key with a passphrase cannot reconnect alone when no SSH agent holds it, so Ralphy does not use it",
        keys_path(HostOs::Windows, true)
    )
}

/// Which OS answers `uname -s`, else `cmd /c ver`.
pub(super) fn detect_os(
    shell: &mut impl HostShell,
    dest: &str,
    identity: Option<&Path>,
    uname: &HostOutput,
) -> Result<HostOs> {
    os_of(dest, uname, || {
        let ver = shell.run(identity, &render(None, &HostOp::WindowsVer)?, b"")?;
        if let Some(kind) = classify(&ver) {
            let why = format!("the connection to {dest} failed: {}", ver.stderr.trim());
            return Err(ssh_error(kind, why));
        }
        Ok(ver)
    })
}

/// The OS from `uname`, the answer to `uname -s`, else from `ver`, the answer
/// to `cmd /c ver`, which is asked only when needed.
pub(crate) fn os_of(
    dest: &str,
    uname: &HostOutput,
    ver: impl FnOnce() -> Result<HostOutput>,
) -> Result<HostOs> {
    match uname.stdout.trim() {
        "Linux" if uname.ok() => return Ok(HostOs::Linux),
        "Darwin" if uname.ok() => return Ok(HostOs::MacOs),
        _ => {}
    }
    if ver()?.stdout.contains("Windows") {
        return Ok(HostOs::Windows);
    }
    bail!(
        "the host {dest} is not a Linux, macOS or Windows host: `uname -s` printed {:?}",
        uname.stdout.trim()
    )
}

fn key_file_refused(dest: &str, key: &Path) -> String {
    format!(
        "the host {dest} refused the key {}. A key with a passphrase works only when an SSH agent holds it",
        key.display()
    )
}

/// Sign in to `dest`: first with the operator's SSH config or agent, then with
/// the peer key, then with the operator's password when the shell has one,
/// which adds the peer key on the host. With `key_file`, only that key is
/// tried. The host key must already be known; an unknown or changed one stops
/// everything before a single command is sent.
pub(crate) fn connect(
    shell: &mut impl HostShell,
    store: &Path,
    dest: &str,
    key_file: Option<&Path>,
    keygen: impl FnOnce(&Path) -> Result<()>,
    out: &mut Report<impl Write>,
) -> Result<(Option<PathBuf>, HostOs)> {
    let uname_cmd = render(None, &HostOp::Uname)?;
    let first = shell.run(key_file, &uname_cmd, b"")?;
    // A key file the operator chose is never swapped for Ralphy's own key.
    if let (Some(k @ SshFailure::AuthRefused), Some(key)) = (classify(&first), key_file) {
        return Err(ssh_error(k, key_file_refused(dest, key)));
    }
    let (identity, uname) = match classify(&first) {
        None => (key_file.map(Path::to_path_buf), first),
        Some(k @ SshFailure::HostKeyUnknown) => return Err(ssh_error(k, unknown_host(dest))),
        Some(k @ SshFailure::HostKeyChanged) => return Err(ssh_error(k, changed_host(dest))),
        Some(SshFailure::AuthRefused) => {
            let key = ensure_peer_key(store, keygen)?;
            let second = shell.run(Some(&key.path), &uname_cmd, b"")?;
            match classify(&second) {
                None => (Some(key.path), second),
                Some(SshFailure::AuthRefused) if shell.has_password() => {
                    add_peer_key(shell, dest, &key.public_line)?;
                    // Not "added": the host is added only by Add host, and
                    // this line shows before the checks.
                    out.note(&format!(
                        "Ralphy installed its SSH key on {dest}. The next connections do not need the password."
                    ))?;
                    let third = shell.run(Some(&key.path), &uname_cmd, b"")?;
                    if let Some(k) = classify(&third) {
                        let why = format!(
                            "the host {dest} took the password but refused the key it has just received: {}",
                            third.stderr.trim()
                        );
                        return Err(ssh_error(k, why));
                    }
                    (Some(key.path), third)
                }
                Some(kind @ SshFailure::AuthRefused) => {
                    return Err(SshError {
                        kind,
                        message: both_refused(dest, &key.public_line),
                        key_line: Some(key.public_line),
                    }
                    .into())
                }
                Some(k) => {
                    let why = format!("the connection to {dest} failed: {}", second.stderr.trim());
                    return Err(ssh_error(k, why));
                }
            }
        }
        Some(k @ SshFailure::Unreachable) => {
            let why = format!("could not reach {dest}: {}", first.stderr.trim());
            return Err(ssh_error(k, why));
        }
        Some(k @ (SshFailure::Other | SshFailure::PasswordRefused | SshFailure::Prompt)) => {
            let why = format!("the connection to {dest} failed: {}", first.stderr.trim());
            return Err(ssh_error(k, why));
        }
    };
    let os = detect_os(shell, dest, identity.as_deref(), &uname)?;
    Ok((identity, os))
}
