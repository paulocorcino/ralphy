//! Pairing: the `ralphy host add`, `check` and `remove` flows over a
//! [`HostShell`]. Nothing is written into the local store until every remote
//! step has succeeded, and nothing from the host is written except the one
//! descriptor.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use ralphy_daemon::peer::key::{key_body, key_path_in};
use ralphy_daemon::peer::{self, PeerDescriptor};
use ralphy_release::Build;

use super::checks::{
    classify_describe, evaluate, parse_facts, CheckStatus, HostCheck, RalphyOnHost,
};
use super::install::{offer_for, Offer};
use super::report::Report;
use super::shell::{render, HostOp, HostOs};
use super::ssh::{classify, ssh_error, HostOutput, HostShell, SshFailure};

mod connect;
mod port;
use connect::{changed_host, detect_os, unknown_host};
pub(crate) use connect::{connect, os_of};
use port::LAST_TUNNEL_PORT;
pub(crate) use port::{choose_local_port, FIRST_TUNNEL_PORT};

/// The checks, the host's Ralphy, the signed-in user, and the install offer.
type Surveyed = (Vec<HostCheck>, RalphyOnHost, Option<String>, Option<Offer>);

/// This computer, as the add flow needs it.
pub(crate) struct Local<'a> {
    pub store: &'a Path,
    pub daemon_id: Option<String>,
    pub name: Option<String>,
    pub port: u16,
    pub build: Build,
    pub target: Option<&'static str>,
}

/// A signed-in session: the key to use (`None` = the operator's SSH config or
/// agent) and the host's OS.
pub(super) struct Session<'s, S: HostShell> {
    pub(super) shell: &'s mut S,
    pub(super) dest: &'s str,
    pub(super) identity: Option<PathBuf>,
    pub(super) os: HostOs,
}

impl<S: HostShell> Session<'_, S> {
    /// Run `op`. An `ssh` failure is an error; the remote command's own exit
    /// code is left to the caller.
    pub(super) fn run(&mut self, op: &HostOp, stdin: &[u8]) -> Result<HostOutput> {
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
    pub(super) fn run_ok(&mut self, op: &HostOp, stdin: &[u8]) -> Result<HostOutput> {
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

/// Probe the host and read its Ralphy, then evaluate the checks and what
/// `ralphy host install` could send.
fn survey<S: HostShell>(
    s: &mut Session<'_, S>,
    local: &Local<'_>,
    wanted_name: Option<&str>,
) -> Result<Surveyed> {
    let probe = s.run(&HostOp::Probe, b"")?;
    let facts = parse_facts(s.os, &probe.stdout);
    let described = s.run(&HostOp::Describe { with_token: false }, b"")?;
    let ralphy = classify_describe(&described)?;
    let host_id = match &ralphy {
        RalphyOnHost::Described(d) => d.daemon_id.clone(),
        _ => None,
    };
    let names = fleet_names(local, host_id.as_deref());
    let mut checks = evaluate(&facts, &ralphy, &names, s.dest, wanted_name);
    let offer = offer_for(&mut checks, &facts, &ralphy, local);
    Ok((checks, ralphy, facts.user, offer))
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
    let (identity, os) = connect(shell, local.store, dest, key_file, keygen, out)?;
    out.connected(os)?;
    let mut s = Session {
        shell,
        dest,
        identity,
        os,
    };
    let (checks, ralphy, user, offer) = survey(&mut s, local, wanted_name)?;
    out.checks(dest, os, &checks, offer.as_ref())?;
    if let RalphyOnHost::Described(d) = &ralphy {
        if is_self(local, d.daemon_id.as_deref()) {
            bail!("the host {dest} is this computer's own daemon");
        }
    }
    if checks.iter().any(HostCheck::is_blocking) {
        bail!("the host {dest} is not ready: do what the checks above say, then run this command again");
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
        bail!("the host {dest} is this computer's own daemon");
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
    let (identity, os) = connect(shell, local.store, dest, key_file, keygen, out)?;
    out.connected(os)?;
    let mut s = Session {
        shell,
        dest,
        identity,
        os,
    };
    let (checks, _, _, offer) = survey(&mut s, local, wanted_name)?;
    out.checks(dest, os, &checks, offer.as_ref())?;
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
