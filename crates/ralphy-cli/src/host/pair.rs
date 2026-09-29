//! Pairing: the `ralphy host add`, `check` and `remove` flows over a
//! [`HostShell`]. Nothing is written into the local store until every remote
//! step has succeeded, and nothing from the host is written except the one
//! descriptor.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use ralphy_daemon::peer::key::{ensure_peer_key, key_body, key_path_in};
use ralphy_daemon::peer::{self, PeerDescriptor};

use super::checks::{
    classify_describe, evaluate, parse_facts, CheckStatus, HostCheck, RalphyOnHost,
};
use super::report::Report;
use super::shell::{keys_path, render, HostOp, HostOs};
use super::ssh::{classify, ssh_error, HostOutput, HostShell, SshError, SshFailure};

/// This computer, as the add flow needs it.
pub(crate) struct Local<'a> {
    pub store: &'a Path,
    pub daemon_id: Option<String>,
    pub name: Option<String>,
    pub port: u16,
}

/// A signed-in session: the key to use (`None` = the operator's SSH config or
/// agent) and the host's OS.
struct Session<'s, S: HostShell> {
    shell: &'s mut S,
    dest: &'s str,
    identity: Option<PathBuf>,
    os: HostOs,
}

impl<S: HostShell> Session<'_, S> {
    /// Run `op`. An `ssh` failure is an error; the remote command's own exit
    /// code is left to the caller.
    fn run(&mut self, op: &HostOp, stdin: &[u8]) -> Result<HostOutput> {
        let command = render(Some(self.os), op)?;
        let out = self.shell.run(self.identity.as_deref(), &command, stdin)?;
        if let Some(kind) = classify(&out) {
            let why = format!(
                "the connection to {} failed: {}",
                self.dest,
                out.stderr.trim()
            );
            return Err(ssh_error(kind, why));
        }
        Ok(out)
    }

    /// Run `op` and fail when the remote command fails.
    fn run_ok(&mut self, op: &HostOp, stdin: &[u8]) -> Result<HostOutput> {
        let out = self.run(op, stdin)?;
        if !out.ok() {
            bail!(
                "`{}` failed on {}: {}",
                render(Some(self.os), op)?,
                self.dest,
                out.stderr.trim()
            );
        }
        Ok(out)
    }
}

fn unknown_host(dest: &str) -> String {
    format!(
        "the host key of {dest} is not known yet, so Ralphy sent nothing to it. Run `ssh {dest}` once, \
         check that the fingerprint it shows is the host's, answer yes, then run this command again"
    )
}

fn changed_host(dest: &str) -> String {
    format!(
        "the host key of {dest} has changed, so Ralphy sent nothing to it. Find out why the key changed \
         before you trust it: `ssh {dest}` shows the details"
    )
}

fn both_refused(dest: &str, public_line: &str) -> String {
    format!(
        "{dest} refused your SSH key and Ralphy's key. Add this line to .ssh/authorized_keys on the host \
         (for a Windows administrator: {}), then run this command again:\n{public_line}\n\
         A key with a passphrase cannot reconnect alone when no SSH agent holds it, so Ralphy does not use it",
        keys_path(HostOs::Windows, true)
    )
}

/// Which OS answers `uname -s`, else `cmd /c ver`.
fn detect_os(
    shell: &mut impl HostShell,
    dest: &str,
    identity: Option<&Path>,
    uname: &HostOutput,
) -> Result<HostOs> {
    match uname.stdout.trim() {
        "Linux" if uname.ok() => return Ok(HostOs::Linux),
        "Darwin" if uname.ok() => return Ok(HostOs::MacOs),
        _ => {}
    }
    let ver = shell.run(identity, &render(None, &HostOp::WindowsVer)?, b"")?;
    if let Some(kind) = classify(&ver) {
        let why = format!("the connection to {dest} failed: {}", ver.stderr.trim());
        return Err(ssh_error(kind, why));
    }
    if ver.stdout.contains("Windows") {
        return Ok(HostOs::Windows);
    }
    bail!(
        "{dest} is not a Linux, macOS or Windows host: `uname -s` printed {:?}",
        uname.stdout.trim()
    )
}

fn key_file_refused(dest: &str, key: &Path) -> String {
    format!(
        "{dest} refused the key {}. A key with a passphrase works only when an SSH agent holds it",
        key.display()
    )
}

/// Sign in to `dest`: first with the operator's SSH config or agent, then with
/// the peer key. With `key_file`, only that key is tried. The host key must
/// already be known; an unknown or changed one stops everything before a
/// single command is sent.
pub(crate) fn connect(
    shell: &mut impl HostShell,
    store: &Path,
    dest: &str,
    key_file: Option<&Path>,
    keygen: impl FnOnce(&Path) -> Result<()>,
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
        Some(k @ SshFailure::Other) => {
            let why = format!("the connection to {dest} failed: {}", first.stderr.trim());
            return Err(ssh_error(k, why));
        }
    };
    let os = detect_os(shell, dest, identity.as_deref(), &uname)?;
    Ok((identity, os))
}

/// Probe the host and read its Ralphy, then evaluate the checks.
fn survey<S: HostShell>(
    s: &mut Session<'_, S>,
    fleet_names: impl FnOnce(Option<&str>) -> Vec<String>,
    wanted_name: Option<&str>,
) -> Result<(Vec<HostCheck>, RalphyOnHost, Option<String>)> {
    let probe = s.run(&HostOp::Probe, b"")?;
    let facts = parse_facts(s.os, &probe.stdout);
    let described = s.run(&HostOp::Describe { with_token: false }, b"")?;
    let ralphy = classify_describe(&described)?;
    let host_id = match &ralphy {
        RalphyOnHost::Described(d) => d.daemon_id.clone(),
        _ => None,
    };
    let names = fleet_names(host_id.as_deref());
    let checks = evaluate(&facts, &ralphy, &names, s.dest, wanted_name);
    Ok((checks, ralphy, facts.user))
}

/// The names of this computer's daemon and of every peer that is not
/// `host_id`, which may keep its own name.
fn fleet_names(local: &Local<'_>, host_id: Option<&str>) -> Vec<String> {
    let (peers, _) = peer::read_store(&local.store.join("peers"));
    local
        .name
        .iter()
        .cloned()
        .chain(
            peers
                .into_iter()
                .filter(|p| Some(p.daemon_id.as_str()) != host_id)
                .map(|p| p.name),
        )
        .collect()
}

fn is_self(local: &Local<'_>, host_id: Option<&str>) -> bool {
    host_id.is_some() && host_id == local.daemon_id.as_deref()
}

/// `ralphy host add`: check the host, apply the fixes, read its token, and
/// write the local descriptor with its tunnel section. Writing the descriptor
/// is the last side effect, so a failure anywhere before it leaves the local
/// store as it was.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add(
    shell: &mut impl HostShell,
    local: &Local<'_>,
    dest: &str,
    key_file: Option<&Path>,
    wanted_name: Option<&str>,
    keygen: impl FnOnce(&Path) -> Result<()>,
    is_free: impl Fn(u16) -> bool,
    out: &mut Report<impl Write>,
) -> Result<PeerDescriptor> {
    let (identity, os) = connect(shell, local.store, dest, key_file, keygen)?;
    out.connected(os)?;
    let mut s = Session {
        shell,
        dest,
        identity,
        os,
    };
    let (checks, ralphy, user) = survey(&mut s, |id| fleet_names(local, id), wanted_name)?;
    out.checks(dest, os, &checks)?;
    if let RalphyOnHost::Described(d) = &ralphy {
        if is_self(local, d.daemon_id.as_deref()) {
            bail!("{dest} is this computer's own daemon");
        }
    }
    if checks.iter().any(HostCheck::is_blocking) {
        bail!("{dest} is not ready: do what the checks above say, then run this command again");
    }
    let running = matches!(&ralphy, RalphyOnHost::Described(d) if d.running);

    let mut restart = !running;
    for check in &checks {
        let CheckStatus::Fix(op) = &check.status else {
            continue;
        };
        let answer = s.run(op, b"")?;
        if !answer.ok() {
            if *op == HostOp::EnableLinger {
                out.linger_needs_sudo(user.as_deref().unwrap_or("<user>"))?;
                continue;
            }
            bail!(
                "`{}` failed on {dest}: {}",
                render(Some(os), op)?,
                answer.stderr.trim()
            );
        }
        out.fixed(check, &render(Some(os), op)?)?;
        if matches!(op, HostOp::RequireTokenOn | HostOp::SetName { .. }) {
            restart = true;
        }
    }
    if restart {
        s.run_ok(&HostOp::Restart, b"")?;
        out.note(&format!("Restarted the daemon on {dest}."))?;
    }

    let described = s.run(&HostOp::Describe { with_token: true }, b"")?;
    let RalphyOnHost::Described(d) = classify_describe(&described)? else {
        bail!("the Ralphy on {dest} stopped answering `ralphy daemon describe`");
    };
    if is_self(local, d.daemon_id.as_deref()) {
        bail!("{dest} is this computer's own daemon");
    }
    let (existing, _) = peer::read_store(&local.store.join("peers"));
    let same = |p: &&PeerDescriptor| Some(p.daemon_id.as_str()) == d.daemon_id.as_deref();
    let keep = existing.iter().find(same).map(|p| p.port);
    let taken: Vec<u16> = existing
        .iter()
        .filter(|p| !same(p))
        .map(|p| p.port)
        .collect();
    let port = choose_local_port(local.port, &taken, keep, is_free).with_context(|| {
        format!("no free local port between {FIRST_TUNNEL_PORT} and {LAST_TUNNEL_PORT}")
    })?;
    let identity_file = s.identity.as_ref().map(|p| p.display().to_string());
    let descriptor = peer::paired_descriptor(&d, dest, port, identity_file)?;
    peer::write_descriptor(local.store, &descriptor)?;
    out.added(&descriptor, dest, port)?;
    Ok(descriptor)
}

/// `ralphy host check`: the same checks as `add`, printed. Changes nothing on
/// the host. When the host refuses the SSH config, it creates this computer's
/// peer key like `add` does, so the failure can show the line to add on the
/// host; the workbench dialog has no other way to get that line.
pub(crate) fn check(
    shell: &mut impl HostShell,
    local: &Local<'_>,
    dest: &str,
    key_file: Option<&Path>,
    wanted_name: Option<&str>,
    keygen: impl FnOnce(&Path) -> Result<()>,
    out: &mut Report<impl Write>,
) -> Result<Vec<HostCheck>> {
    let (identity, os) = connect(shell, local.store, dest, key_file, keygen)?;
    out.connected(os)?;
    let mut s = Session {
        shell,
        dest,
        identity,
        os,
    };
    let (checks, _, _) = survey(&mut s, |id| fleet_names(local, id), wanted_name)?;
    out.checks(dest, os, &checks)?;
    Ok(checks)
}

/// The accepted tunnel peer whose name or daemon id is `name_or_id`.
pub(crate) fn find_host(store: &Path, name_or_id: &str) -> Result<PeerDescriptor> {
    let (peers, _) = peer::read_store(&store.join("peers"));
    let hosts: Vec<PeerDescriptor> = peers.into_iter().filter(|p| p.tunnel.is_some()).collect();
    if let Some(found) = hosts
        .iter()
        .find(|p| p.name == name_or_id || p.daemon_id == name_or_id)
    {
        return Ok(found.clone());
    }
    let names: Vec<&str> = hosts.iter().map(|p| p.name.as_str()).collect();
    if names.is_empty() {
        bail!("no host is named {name_or_id}: this computer has no hosts")
    }
    bail!(
        "no host is named {name_or_id}. The hosts are: {}",
        names.join(", ")
    )
}

/// The peer key's public line, when this computer has a peer key.
fn peer_public_line(store: &Path) -> Result<Option<String>> {
    let public = PathBuf::from(format!("{}.pub", key_path_in(store).display()));
    match std::fs::read_to_string(&public) {
        Ok(text) => Ok(Some(text.trim().to_string())),
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
        if rotate_token {
            let why = format!(
                "{silent}. Its access token was not changed, and Ralphy still knows {name}"
            );
            return Err(ssh_error(kind, why));
        }
        forget(store, host)?;
        out.note(&format!("Forgot {name} on this computer."))?;
        return match public_line {
            Some(line) => Err(ssh_error(kind, format!(
                "{silent}. The key line remains on {name}: remove this line from its authorized keys file:\n{line}"
            ))),
            None => out.note(&format!("{silent}. This computer has no key of its own there.")),
        };
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
            "Changed the access token of {name}. Every other computer connected to {name} is now disconnected."
        ))?;
    }

    match public_line.as_deref().and_then(key_body) {
        None => out.note(&format!("This computer has no key of its own on {name}."))?,
        Some(body) => {
            let admin = os == HostOs::Windows && {
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
        }
    }

    forget(store, host)?;
    out.note(&format!("Forgot {name} on this computer."))?;
    out.note("The open tunnel closes when the daemon restarts.")?;
    Ok(())
}

/// The first local port a tunnel takes. The range stays clear of the daemon's
/// default port, 7257.
pub(crate) const FIRST_TUNNEL_PORT: u16 = 7401;
const LAST_TUNNEL_PORT: u16 = 7499;

/// The local end of a new tunnel. A re-added daemon keeps its old port, so the
/// descriptor does not move; otherwise the first free port that is neither the
/// local daemon's nor another descriptor's.
pub(crate) fn choose_local_port(
    daemon_port: u16,
    taken: &[u16],
    keep: Option<u16>,
    is_free: impl Fn(u16) -> bool,
) -> Option<u16> {
    if let Some(port) = keep.filter(|p| *p != daemon_port) {
        return Some(port);
    }
    (FIRST_TUNNEL_PORT..=LAST_TUNNEL_PORT)
        .find(|p| *p != daemon_port && !taken.contains(p) && is_free(*p))
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

#[cfg(test)]
mod tests;
