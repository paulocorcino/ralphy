//! `ralphy host remove`: delete this computer's key line on the host and forget the descriptor.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ralphy_daemon::peer::key::{key_body, key_path_in};
use ralphy_daemon::peer::PeerDescriptor;

use super::super::report::Report;
use super::super::shell::{render, HostOp, HostOs};
use super::super::ssh::{classify, ssh_error, HostShell, SshFailure};
use super::connect::{changed_host, detect_os, unknown_host};
use super::Session;

/// The peer key's public line, when this computer has a peer key.
fn peer_public_line(store: &Path) -> Result<Option<String>> {
    let public = PathBuf::from(format!("{}.pub", key_path_in(store).display()));
    match std::fs::read_to_string(&public) {
        Ok(text) => Ok(Some(text.trim().to_owned())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", public.display())),
    }
}

fn forget(store: &Path, host: &PeerDescriptor) -> Result<()> {
    let path = store.join("peers").join(format!("{}.toml", host.daemon_id));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("deleting {}", path.display())),
    }
}

/// `ralphy host remove`: delete this computer's key line on the host and
/// forget the descriptor. The host daemon keeps running; with `rotate_token`
/// its token changes, which disconnects every other computer that uses it.
pub(crate) fn remove(
    shell: &mut impl HostShell,
    store: &Path,
    host: &PeerDescriptor,
    rotate_token: bool,
    out: &mut Report<impl Write>,
) -> Result<()> {
    let name = host.name.as_str();
    let tunnel = host
        .tunnel
        .as_ref()
        .with_context(|| format!("{name} is not reached through a tunnel"))?;
    let dest = tunnel.destination.as_str();
    let identity = tunnel.identity_file.as_ref().map(PathBuf::from);
    let public_line = peer_public_line(store)?;

    let uname = shell.run(identity.as_deref(), &render(None, &HostOp::Uname)?, b"")?;
    match classify(&uname) {
        Some(k @ SshFailure::HostKeyUnknown) => return Err(ssh_error(k, unknown_host(dest))),
        Some(k @ SshFailure::HostKeyChanged) => return Err(ssh_error(k, changed_host(dest))),
        _ => {}
    }
    if let Some(kind) = classify(&uname) {
        let silent = format!("{name} did not answer: {}", uname.stderr.trim());
        let unreached = Unreached {
            kind,
            silent,
            public_line,
        };
        return remove_unreached(store, host, rotate_token, unreached, out);
    }
    let os = detect_os(shell, dest, identity.as_deref(), &uname)?;
    let mut s = Session {
        shell,
        dest,
        identity,
        os,
    };

    // Rotate before the key line goes: the peer key may be what signs in.
    if rotate_token {
        s.run_ok(&HostOp::RotateToken, b"")?;
        s.run_ok(&HostOp::Restart, b"")?;
        out.note(&format!(
            "Changed the access token of {name}. \
             Every other computer connected to {name} is now disconnected."
        ))?;
    }

    match public_line.as_deref().and_then(key_body) {
        None => out.note(&format!("This computer has no key of its own on {name}."))?,
        Some(body) => remove_key_line(&mut s, name, body, out)?,
    }

    forget(store, host)?;
    out.note(&format!("Forgot {name} on this computer."))?;
    out.note("The open tunnel closes when the daemon restarts.")?;
    Ok(())
}

/// A host that `ralphy host remove` could not reach: the SSH failure, the
/// message that says so, and this computer's key line that may remain there.
struct Unreached {
    kind: SshFailure,
    silent: String,
    public_line: Option<String>,
}

/// `ralphy host remove` for a host that did not answer: with a token rotation
/// it fails and Ralphy keeps the host; otherwise the descriptor is forgotten
/// and the operator is told which key line to remove by hand.
fn remove_unreached(
    store: &Path,
    host: &PeerDescriptor,
    rotate_token: bool,
    unreached: Unreached,
    out: &mut Report<impl Write>,
) -> Result<()> {
    let name = host.name.as_str();
    let Unreached {
        kind,
        silent,
        public_line,
    } = unreached;
    if rotate_token {
        let why =
            format!("{silent}. Its access token was not changed, and Ralphy still knows {name}");
        return Err(ssh_error(kind, why));
    }
    forget(store, host)?;
    out.note(&format!("Forgot {name} on this computer."))?;
    match public_line {
        Some(line) => Err(ssh_error(
            kind,
            format!(
                "{silent}. The key line remains on {name}: remove this line from its \
             authorized keys file:\n{line}"
            ),
        )),
        None => out.note(&format!(
            "{silent}. This computer has no key of its own there."
        )),
    }
}

/// Delete every line of the host's authorized keys file that holds `body`.
/// An administrator on Windows has its keys in another file.
fn remove_key_line<S: HostShell>(
    s: &mut Session<'_, S>,
    name: &str,
    body: &str,
    out: &mut Report<impl Write>,
) -> Result<()> {
    let admin = s.os == HostOs::Windows && {
        let probe = s.run(&HostOp::Probe, b"")?;
        probe.stdout.contains("S-1-5-32-544")
    };
    let keys = s.run_ok(&HostOp::ReadKeys { admin }, b"")?;
    match without_key_line(&keys.stdout, body) {
        None => out.note(&format!("This computer's key line is not on {name}."))?,
        Some(rest) if rest.trim().is_empty() => {
            s.run_ok(&HostOp::ClearKeys { admin }, b"")?;
            out.note(&format!("Removed this computer's key line on {name}."))?;
        }
        Some(rest) => {
            s.run_ok(&HostOp::WriteKeys { admin }, rest.as_bytes())?;
            out.note(&format!("Removed this computer's key line on {name}."))?;
        }
    }
    Ok(())
}

/// `text` without every line that holds `body` as one whole field. A key line
/// may start with options (`restrict,port-forwarding ssh-ed25519 …`), so the
/// body is not always the second field; a longer key that only contains the
/// body is kept. Line endings are kept byte for byte. `None` when no line
/// matched.
pub(crate) fn without_key_line(text: &str, body: &str) -> Option<String> {
    let mut kept = String::with_capacity(text.len());
    let mut dropped = false;
    for line in text.split_inclusive('\n') {
        if line.split_whitespace().any(|field| field == body) {
            dropped = true;
        } else {
            kept.push_str(line);
        }
    }
    dropped.then_some(kept)
}
