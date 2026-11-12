//! The local fleet's peer descriptor (docs/adr/0052 §3): what a daemon announces
//! about itself into a directory another daemon can read — its identity triple,
//! the loopback address it bound, a human environment label, its OWN access
//! token, and the peer protocol version it speaks.
//!
//! Mirrors `registry`: pure sync, path-explicit, no `ralphy-core`. The fold is a
//! pure function over `(file_name, file_text)` pairs so the degradation rules
//! (malformed, incompatible, duplicate) are testable without touching a disk;
//! [`read_store`] is the thin I/O shell around it.
//!
//! A descriptor is a CLAIM, not a fact: nothing here proves the peer is up. That
//! is `peer::client`'s job, computed fresh on every request and never persisted.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub mod client;
mod environment;
pub(crate) mod join;
pub mod key;
pub mod nudge;
pub(crate) mod telegram_claim;
pub mod tunnel;

pub use environment::{
    detect_environment, environment_label, linux_release, macos_release, system_release,
};

#[cfg(test)]
mod tests;

/// The peer handshake protocol version this daemon speaks. Two daemons upgrade
/// independently (ADR-0052 §3), so this — not the descriptor's field set — is
/// the compatibility gate. A change to the shape of a reply that a peer can
/// answer raises it, unless each change is a new field the page reads as
/// optional (ADR-0052, amendment of 2026-10-10). The pin in
/// `crates/xtask/tests/ratchets/` fails until the change makes one of the two.
/// Version 5 moves a relayed console and a relayed spawn verb onto a channel
/// of the peer's page socket `/ws`: a peer of version 4 takes that socket but
/// ignores `channel.open`, so without the raise the relay would wait forever
/// instead of naming the older peer.
pub const PEER_PROTOCOL_VERSION: u32 = 5;

/// How to wake a peer that is not answering. Only ever populated by a daemon
/// running inside WSL, which is the one environment whose host can start it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NudgeSpec {
    pub distro: String,
    pub unit: String,
}

/// How to reach a peer on another machine: an `ssh` local forward that the
/// local daemon holds open (ADR-0067 §2). Written by hand or by the add flow,
/// never announced by the peer itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelSpec {
    /// An `~/.ssh/config` alias or `user@host`.
    pub destination: String,
    /// The peer's TCP port. 0 when `peer_socket` names the destination.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub peer_port: u16,
    /// The absolute path of the peer account's socket on the host. Set instead
    /// of `peer_port` when the host reports a socket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_socket: Option<String>,
    /// Always equal to the descriptor's `port`: the local end is what we dial.
    pub local_port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if calls it with a reference to the field"
)]
fn is_zero(n: &u16) -> bool {
    *n == 0
}

/// One daemon's self-announcement, written as `<store>/peers/<daemon_id>.toml`.
///
/// NOT `deny_unknown_fields` on purpose: a newer peer announcing an extra field
/// must be usable by an older reader, so the `protocol_version` gate is the only
/// compatibility check (ADR-0052 §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerDescriptor {
    pub daemon_id: String,
    pub name: String,
    pub avatar: String,
    pub address: String,
    pub port: u16,
    pub environment: String,
    /// The peer's OS family (`std::env::consts::OS`: `windows`, `linux`,
    /// `macos`). It picks the workbench icon; the `environment` label is for
    /// people. Empty in a descriptor written before the field existed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub os: String,
    pub token: String,
    pub protocol_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudge: Option<NudgeSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<TunnelSpec>,
}

/// What a host daemon says about itself to a computer that pairs with it
/// (`ralphy daemon describe`). The producer and the consumer share this one
/// type, so the field set cannot drift between them.
///
/// NOT `deny_unknown_fields`, for the same reason as [`PeerDescriptor`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonDescription {
    pub daemon_id: Option<String>,
    pub name: Option<String>,
    pub avatar: Option<String>,
    pub environment: String,
    pub os: String,
    pub port: u16,
    pub protocol_version: u32,
    pub require_token: bool,
    pub autostart: bool,
    pub running: bool,
    /// The absolute path of the daemon's socket, set only when a connection
    /// to it succeeded. Optional: an older daemon and Windows never set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// The local descriptor for a host reached through an `ssh` local forward:
/// the local end `127.0.0.1:<local_port>` is what the fleet dials.
pub fn paired_descriptor(
    d: &DaemonDescription,
    destination: &str,
    local_port: u16,
    identity_file: Option<String>,
) -> Result<PeerDescriptor> {
    let daemon_id = d
        .daemon_id
        .clone()
        .context("the host daemon has no identity")?;
    // The id names the descriptor file, and it comes from the other machine.
    if daemon_id.parse::<ulid::Ulid>().is_err() {
        anyhow::bail!("the host daemon's identity {daemon_id:?} is not a valid id");
    }
    let name = d.name.clone().context("the host daemon has no name")?;
    let token = d
        .token
        .clone()
        .context("the host daemon did not give its access token")?;
    Ok(PeerDescriptor {
        daemon_id,
        name,
        avatar: d.avatar.clone().unwrap_or_default(),
        address: "127.0.0.1".to_owned(),
        port: local_port,
        environment: d.environment.clone(),
        os: d.os.clone(),
        token,
        protocol_version: d.protocol_version,
        nudge: None,
        tunnel: Some(TunnelSpec {
            destination: destination.to_owned(),
            peer_port: if d.socket.is_some() { 0 } else { d.port },
            peer_socket: d.socket.clone(),
            local_port,
            identity_file,
        }),
    })
}

/// Why one descriptor record was not usable. Degradation is per-record: a
/// rejection never removes an accepted peer, and never fails the fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerReject {
    Malformed {
        file: String,
        why: String,
    },
    IncompatibleVersion {
        file: String,
        daemon_id: String,
        environment: String,
        theirs: u32,
    },
    DuplicateIdentity {
        file: String,
        daemon_id: String,
    },
    /// The store directory, or one of its entries, exists but cannot be read
    /// (ADR-0070 D4: a store the daemon cannot read is a failure, never "no
    /// peers").
    Unreadable {
        path: String,
        why: String,
    },
}

impl PeerReject {
    /// The file this rejection is about — the operator's handle on it.
    pub fn file(&self) -> &str {
        match self {
            PeerReject::Malformed { file, .. }
            | PeerReject::IncompatibleVersion { file, .. }
            | PeerReject::DuplicateIdentity { file, .. } => file,
            PeerReject::Unreadable { path, .. } => path,
        }
    }

    /// Operator-facing sentences naming the file and the reason. The workbench
    /// shows them as they are, so they follow ADR-0065 (#432).
    pub fn why(&self) -> String {
        match self {
            PeerReject::Malformed { file, why } => {
                format!("{file} is not a peer descriptor: {why}.")
            }
            PeerReject::IncompatibleVersion { file, theirs, .. } => format!(
                "{file} speaks peer protocol {theirs}, and this daemon speaks \
                 {PEER_PROTOCOL_VERSION}. Upgrade the older Ralphy."
            ),
            PeerReject::DuplicateIdentity { file, daemon_id } => {
                format!(
                    "{file} announces daemon {daemon_id} again. \
                     An earlier file already announced it."
                )
            }
            PeerReject::Unreadable { path, why } => {
                format!("The peer store {path} cannot be read: {why}.")
            }
        }
    }

    /// Return the environment and version for an incompatible daemon identity.
    pub fn version_mismatch_for(&self, daemon_id: &str) -> Option<(&str, u32)> {
        match self {
            PeerReject::IncompatibleVersion {
                daemon_id: rejected_id,
                environment,
                theirs,
                ..
            } if rejected_id == daemon_id => Some((environment, *theirs)),
            PeerReject::Malformed { .. }
            | PeerReject::IncompatibleVersion { .. }
            | PeerReject::DuplicateIdentity { .. }
            | PeerReject::Unreadable { .. } => None,
        }
    }
}

/// Whether the protocol version saved in a descriptor refuses it.
///
/// A self-announced descriptor is rewritten at each start of its daemon, so
/// its version is current. A tunnel descriptor keeps the version read at
/// `host add`, and the host can be upgraded after it: the handshake checks it
/// on connect (ADR-0052 §3), and the host stays in the list to be upgraded or
/// removed.
fn saved_version_refused(d: &PeerDescriptor) -> bool {
    d.tunnel.is_none() && d.protocol_version != PEER_PROTOCOL_VERSION
}

/// Fold `(file_name, file_text)` records — caller-sorted by file name — into the
/// usable peers plus the per-record rejections. Pure: no I/O, so every
/// degradation rule is unit-testable.
///
/// The FIRST record claiming a `daemon_id` wins; every later one is rejected
/// without disturbing the accepted list.
pub fn fold(records: &[(String, String)]) -> (Vec<PeerDescriptor>, Vec<PeerReject>) {
    let mut accepted: Vec<PeerDescriptor> = Vec::new();
    let mut rejected: Vec<PeerReject> = Vec::new();
    for (file, text) in records {
        let d = match read_record(file, text) {
            Ok(d) => d,
            Err(reject) => {
                rejected.push(reject);
                continue;
            }
        };
        if accepted.iter().any(|a| a.daemon_id == d.daemon_id) {
            rejected.push(PeerReject::DuplicateIdentity {
                file: file.clone(),
                daemon_id: d.daemon_id,
            });
            continue;
        }
        accepted.push(d);
    }
    (accepted, rejected)
}

/// One record as a descriptor, or the reason it is refused on its own,
/// before it is compared with the other records.
fn read_record(file: &str, text: &str) -> Result<PeerDescriptor, PeerReject> {
    let d: PeerDescriptor = toml::from_str(text).map_err(|e| PeerReject::Malformed {
        file: file.to_owned(),
        why: e.to_string(),
    })?;
    if saved_version_refused(&d) {
        return Err(PeerReject::IncompatibleVersion {
            file: file.to_owned(),
            daemon_id: d.daemon_id,
            environment: d.environment,
            theirs: d.protocol_version,
        });
    }
    if let Some(why) = d.tunnel.as_ref().and_then(|t| tunnel_flaw(t, d.port)) {
        return Err(PeerReject::Malformed {
            file: file.to_owned(),
            why,
        });
    }
    Ok(d)
}

/// What is wrong in a tunnel of a descriptor whose port is `port`, if
/// anything.
fn tunnel_flaw(t: &TunnelSpec, port: u16) -> Option<String> {
    if t.local_port != port {
        return Some(format!(
            "its tunnel local port {} is not its port {}",
            t.local_port, port
        ));
    }
    let bad_socket = t
        .peer_socket
        .as_deref()
        .is_some_and(|s| !s.starts_with('/'));
    let has_target = t.peer_port != 0 || t.peer_socket.is_some();
    if t.destination.trim().is_empty() || !has_target || bad_socket || t.local_port == 0 {
        return Some(format!(
            "its tunnel needs a destination, a local port that is not 0, and a peer port that is \
             not 0 or an absolute peer socket path (destination `{}`, peer port {}, \
             peer socket {:?}, local port {})",
            t.destination, t.peer_port, t.peer_socket, t.local_port
        ));
    }
    None
}

/// Read every `*.toml` in `dir`, sorted by file name, and [`fold`] them. A
/// missing directory is not an error — it is a fleet of one. A directory or an
/// entry that cannot be read is a [`PeerReject::Unreadable`], so the fleet
/// view shows the failure instead of an empty fleet.
pub fn read_store(dir: &Path) -> (Vec<PeerDescriptor>, Vec<PeerReject>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), Vec::new()),
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "failed to read the peer store");
            let unreadable = PeerReject::Unreadable {
                path: dir.display().to_string(),
                why: e.to_string(),
            };
            return (Vec::new(), vec![unreadable]);
        }
    };
    let mut records: Vec<(String, String)> = Vec::new();
    let mut rejected: Vec<PeerReject> = Vec::new();
    for (n, entry) in entries.enumerate() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                // The fleet row's id is the path: an entry number keeps two
                // failures in one directory from collapsing into one row.
                rejected.push(PeerReject::Unreadable {
                    path: format!("{} (entry {})", dir.display(), n + 1),
                    why: e.to_string(),
                });
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let file = entry.file_name().to_string_lossy().into_owned();
        match std::fs::read_to_string(&path) {
            Ok(text) => records.push((file, text)),
            Err(e) => rejected.push(PeerReject::Malformed {
                file,
                why: e.to_string(),
            }),
        }
    }
    records.sort_by(|a, b| a.0.cmp(&b.0));
    let (accepted, mut folded) = fold(&records);
    rejected.append(&mut folded);
    (accepted, rejected)
}

/// Read the peer store off the reactor. A panic inside `read_store` degrades to
/// "no peers announced", so it is LOGGED rather than swallowed — silently
/// shorter is exactly the failure the fleet view must never show.
pub(crate) async fn read_peer_store(dir: PathBuf) -> (Vec<PeerDescriptor>, Vec<PeerReject>) {
    match tokio::task::spawn_blocking(move || read_store(&dir)).await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!(error = %e, "reading the peer store failed; serving no peers");
            (Vec::new(), Vec::new())
        }
    }
}

/// Write `d` as `<store_dir>/peers/<daemon_id>.toml`, creating the directory and
/// restricting the file to the owner. Returns the written path.
///
/// The owner-only call is best-effort by construction: on a `/mnt/c` drvfs mount
/// (the WSL→Windows announce path) 9p silently ignores the mode, leaving only the
/// Windows profile ACL (ADR-0052 §3).
pub fn write_descriptor(store_dir: &Path, d: &PeerDescriptor) -> Result<PathBuf> {
    let peers = store_dir.join("peers");
    std::fs::create_dir_all(&peers)
        .with_context(|| format!("creating the peer store {}", peers.display()))?;
    let path = peers.join(format!("{}.toml", d.daemon_id));
    let text = toml::to_string_pretty(d).context("serializing the peer descriptor")?;
    crate::owner_only::write_owner_only(&path, text.as_bytes())?;
    Ok(path)
}
