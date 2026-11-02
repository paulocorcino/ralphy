//! Pairing: the `ralphy host add`, `check` and `remove` flows over a
//! [`HostShell`]. Nothing is written into the local store until every remote
//! step has succeeded, and nothing from the host is written except the one
//! descriptor.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use ralphy_daemon::peer::{self, DaemonDescription, PeerDescriptor, TunnelSpec};
use ralphy_release::Build;

use super::checks::{
    classify_describe, evaluate, parse_doctor, parse_facts, with_doctor, CheckStatus, HostCheck,
    RalphyOnHost,
};
use super::install::{offer_for, Offer};
use super::report::Report;
use super::shell::{render, HostOp, HostOs};
use super::ssh::{classify, ssh_error, HostOutput, HostShell};

mod connect;
mod port;
mod remove;
pub(crate) use connect::{connect, os_of};
use port::LAST_TUNNEL_PORT;
pub(crate) use port::{choose_local_port, FIRST_TUNNEL_PORT};
pub(crate) use remove::remove;
#[cfg(test)]
use remove::without_key_line;

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
    /// The newest published release tag, read only for a development build.
    pub latest: Option<String>,
}

/// Where a host flow connects: the destination to pass to `ssh`, and the key
/// to use (`None` = the operator's SSH config or agent).
#[derive(Clone, Copy)]
pub(crate) struct Destination<'a> {
    pub dest: &'a str,
    pub key_file: Option<&'a Path>,
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
        RalphyOnHost::Missing | RalphyOnHost::Old { .. } => None,
    };
    let names = fleet_names(local, host_id.as_deref());
    let mut checks = evaluate(&facts, &ralphy, &names, s.dest, wanted_name);
    let doctor = match ralphy {
        RalphyOnHost::Missing => None,
        RalphyOnHost::Old { .. } | RalphyOnHost::Described(_) => {
            parse_doctor(&s.run(&HostOp::Doctor, b"")?)
        }
    };
    with_doctor(&mut checks, doctor.as_ref(), s.os);
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

/// The `daemon_id` that answers through a tunnel opened with this spec and the
/// host's access token, or why nothing answered.
pub(crate) type Probe<'a> = &'a dyn Fn(&TunnelSpec, Option<&str>) -> Result<String, String>;

/// `ralphy host add`: check the host, apply the fixes, read its token, and
/// write the local descriptor with its tunnel section. Writing the descriptor
/// is the last side effect, so a failure anywhere before it leaves the local
/// store as it was.
#[expect(
    clippy::too_many_arguments,
    reason = "each input of the add flow is a separate thing that the tests replace"
)]
pub(crate) fn add(
    shell: &mut impl HostShell,
    local: &Local<'_>,
    dest: &str,
    key_file: Option<&Path>,
    wanted_name: Option<&str>,
    keygen: impl FnOnce(&Path) -> Result<()>,
    is_free: impl Fn(u16) -> bool,
    probe: Probe<'_>,
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
    refuse_unpairable(local, dest, &ralphy, &checks)?;
    let fix_needs_restart = apply_fixes(&mut s, &checks, user.as_deref(), out)?;
    if needs_restart(&ralphy, os) || fix_needs_restart {
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
    let descriptor = tunnel_descriptor(&d, dest, port, identity_file, probe, out)?;
    peer::write_descriptor(local.store, &descriptor)?;
    out.added(&descriptor, dest, port)?;
    Ok(descriptor)
}

/// The host cannot be paired: it is this computer's own daemon, or a check
/// blocks.
fn refuse_unpairable(
    local: &Local<'_>,
    dest: &str,
    ralphy: &RalphyOnHost,
    checks: &[HostCheck],
) -> Result<()> {
    if let RalphyOnHost::Described(d) = ralphy {
        if is_self(local, d.daemon_id.as_deref()) {
            bail!("the host {dest} is this computer's own daemon");
        }
    }
    if checks.iter().any(HostCheck::is_blocking) {
        bail!(
            "the host {dest} is not ready: do what the checks above say, \
             then run this command again"
        );
    }
    Ok(())
}

/// Must the daemon restart whatever the fixes are? A daemon that is not
/// running, or that runs without a socket on a Unix host, is an older daemon:
/// the restart brings up the socket that the descriptor should name
/// (ADR-0067 amendment M3).
fn needs_restart(ralphy: &RalphyOnHost, os: HostOs) -> bool {
    let restart_needed =
        |d: &DaemonDescription| !d.running || (os != HostOs::Windows && d.socket.is_none());
    !matches!(ralphy, RalphyOnHost::Described(d) if !restart_needed(d))
}

/// Run every fix of `checks`, in order. Returns whether a fix needs the
/// daemon to restart. A linger fix that fails is not fatal: the report names
/// the command to run with `sudo`.
fn apply_fixes<S: HostShell>(
    s: &mut Session<'_, S>,
    checks: &[HostCheck],
    user: Option<&str>,
    out: &mut Report<impl Write>,
) -> Result<bool> {
    let mut restart = false;
    for check in checks {
        let CheckStatus::Fix(op) = &check.status else {
            continue;
        };
        let answer = s.run(op, b"")?;
        if !answer.ok() {
            if *op == HostOp::EnableLinger {
                out.linger_needs_sudo(user.unwrap_or("<user>"))?;
                continue;
            }
            bail!(
                "`{}` failed on {}: {}",
                render(Some(s.os), op)?,
                s.dest,
                answer.stderr.trim()
            );
        }
        out.fixed(check, &render(Some(s.os), op)?)?;
        if matches!(op, HostOp::RequireTokenOn | HostOp::SetName { .. }) {
            restart = true;
        }
    }
    Ok(restart)
}

/// The descriptor of the host, naming the daemon's socket when it answers
/// there and its port otherwise.
fn tunnel_descriptor(
    d: &DaemonDescription,
    dest: &str,
    port: u16,
    identity_file: Option<String>,
    probe: Probe<'_>,
    out: &mut Report<impl Write>,
) -> Result<PeerDescriptor> {
    let descriptor = peer::paired_descriptor(d, dest, port, identity_file.clone())?;
    let Err(why) = reached_through_socket(&descriptor, d, probe) else {
        return Ok(descriptor);
    };
    let by_port = DaemonDescription {
        socket: None,
        ..d.clone()
    };
    let descriptor = peer::paired_descriptor(&by_port, dest, port, identity_file)?;
    port_fallback(&descriptor, d, &why, probe)?;
    out.note(&format!(
        "The host does not forward a connection to the daemon's socket ({why}). \
         This computer reaches the daemon through its port {}.",
        d.port
    ))?;
    Ok(descriptor)
}

/// `Ok` when the descriptor names no socket, or when its daemon answers
/// through it. An `sshd` with `AllowStreamLocalForwarding no` accepts the
/// tunnel and refuses each connection, so only a request shows it (ADR-0067
/// amendment 2026-10-02).
fn reached_through_socket(
    descriptor: &PeerDescriptor,
    d: &DaemonDescription,
    probe: Probe<'_>,
) -> Result<(), String> {
    let Some(tunnel) = descriptor
        .tunnel
        .as_ref()
        .filter(|t| t.peer_socket.is_some())
    else {
        return Ok(());
    };
    match probe(tunnel, d.token.as_deref()) {
        Ok(id) if Some(id.as_str()) == d.daemon_id.as_deref() => Ok(()),
        Ok(id) => Err(format!("another daemon answered: {id}")),
        Err(why) => Err(why),
    }
}

/// The port form is used only when this account's daemon answers on the port.
/// On a host with several accounts the port may belong to another account.
fn port_fallback(
    descriptor: &PeerDescriptor,
    d: &DaemonDescription,
    socket_why: &str,
    probe: Probe<'_>,
) -> Result<()> {
    let Some(tunnel) = descriptor.tunnel.as_ref() else {
        bail!("the descriptor of the host has no tunnel");
    };
    match probe(tunnel, d.token.as_deref()) {
        Ok(id) if Some(id.as_str()) == d.daemon_id.as_deref() => Ok(()),
        Ok(id) => bail!(
            "the host does not forward a connection to the daemon's socket ({socket_why}), \
             and its port {} belongs to another daemon ({id}): ask the host's administrator \
             to set `AllowStreamLocalForwarding yes` for sshd",
            d.port
        ),
        Err(why) => bail!(
            "the host does not forward a connection to the daemon's socket ({socket_why}), \
             and its port {} does not reach this account's daemon ({why}): ask the host's \
             administrator to set `AllowStreamLocalForwarding yes` for sshd",
            d.port
        ),
    }
}

/// `ralphy host check`: the same checks as `add`, printed. Changes nothing on
/// the host. When the host refuses the SSH config, it creates this computer's
/// peer key like `add` does, so the failure can show the line to add on the
/// host; the workbench dialog has no other way to get that line.
pub(crate) fn check(
    shell: &mut impl HostShell,
    local: &Local<'_>,
    destination: Destination<'_>,
    wanted_name: Option<&str>,
    keygen: impl FnOnce(&Path) -> Result<()>,
    out: &mut Report<impl Write>,
) -> Result<Vec<HostCheck>> {
    let Destination { dest, key_file } = destination;
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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod doctor_tests;
