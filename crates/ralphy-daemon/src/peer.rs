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
pub mod nudge;

#[cfg(test)]
mod tests;

/// The peer handshake protocol version this daemon speaks. Two daemons upgrade
/// independently (ADR-0052 §3), so this — not the descriptor's field set — is
/// the compatibility gate.
pub const PEER_PROTOCOL_VERSION: u32 = 3;

/// How to wake a peer that is not answering. Only ever populated by a daemon
/// running inside WSL, which is the one environment whose host can start it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NudgeSpec {
    pub distro: String,
    pub unit: String,
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
    pub token: String,
    pub protocol_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudge: Option<NudgeSpec>,
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
}

impl PeerReject {
    /// The file this rejection is about — the operator's handle on it.
    pub fn file(&self) -> &str {
        match self {
            PeerReject::Malformed { file, .. }
            | PeerReject::IncompatibleVersion { file, .. }
            | PeerReject::DuplicateIdentity { file, .. } => file,
        }
    }

    /// An operator-facing one-liner naming the file and the reason.
    pub fn why(&self) -> String {
        match self {
            PeerReject::Malformed { file, why } => format!("{file} is not a peer descriptor: {why}"),
            PeerReject::IncompatibleVersion { file, theirs, .. } => format!(
                "{file} speaks peer protocol {theirs}, this daemon speaks {PEER_PROTOCOL_VERSION} — upgrade the older one"
            ),
            PeerReject::DuplicateIdentity { file, daemon_id } => {
                format!("{file} re-announces daemon {daemon_id}, already claimed by an earlier file")
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
            _ => None,
        }
    }
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
        let d: PeerDescriptor = match toml::from_str(text) {
            Ok(d) => d,
            Err(e) => {
                rejected.push(PeerReject::Malformed {
                    file: file.clone(),
                    why: e.to_string(),
                });
                continue;
            }
        };
        if d.protocol_version != PEER_PROTOCOL_VERSION {
            rejected.push(PeerReject::IncompatibleVersion {
                file: file.clone(),
                daemon_id: d.daemon_id,
                environment: d.environment,
                theirs: d.protocol_version,
            });
            continue;
        }
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

/// Read every `*.toml` in `dir`, sorted by file name, and [`fold`] them. A
/// missing directory is not an error — it is a fleet of one.
pub fn read_store(dir: &Path) -> (Vec<PeerDescriptor>, Vec<PeerReject>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(dir = %dir.display(), error = %e, "failed to read the peer store; serving no peers");
            }
            return (Vec::new(), Vec::new());
        }
    };
    let mut records: Vec<(String, String)> = Vec::new();
    let mut rejected: Vec<PeerReject> = Vec::new();
    for entry in entries.flatten() {
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

/// The human label for an environment: the WSL distro when running inside one,
/// else a presentable OS name. Pure so the WSL branch is testable off Windows.
pub fn environment_label(wsl_distro: Option<&str>, os: &str) -> String {
    match wsl_distro {
        Some(d) => format!("WSL: {d}"),
        None => match os {
            "windows" => "Windows".to_string(),
            "linux" => "Linux".to_string(),
            "macos" => "macOS".to_string(),
            other => other.to_string(),
        },
    }
}

/// This process's environment label, from `WSL_DISTRO_NAME` and the target OS.
pub fn detect_environment() -> String {
    let distro = std::env::var("WSL_DISTRO_NAME")
        .ok()
        .filter(|d| !d.is_empty());
    environment_label(distro.as_deref(), std::env::consts::OS)
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
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    crate::registry::set_owner_only(&path)?;
    Ok(path)
}
