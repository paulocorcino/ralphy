//! This daemon's peers at start: the announced descriptor, the keepalives and
//! the tunnels.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use crate::{auth, autostart, identity, peer};

/// Start the tasks that hold the peers: the WSL keepalives and the tunnels.
/// Each one starts a child process.
pub(super) fn spawn_peer_tasks(peers_dir: PathBuf, self_id: Option<String>, port: u16) {
    // Hold every nudgeable peer's distro open from the start (ADR-0052 §4
    // amendment): a WSL peer whose keepalive died with the previous daemon
    // would otherwise stay `asleep` until a chip is clicked. Windows only —
    // the handle is a `wsl.exe`. The store is read fresh, the same way the
    // fleet route reads it.
    if cfg!(windows) {
        tokio::spawn(wake_fleet_at_start(peers_dir.clone(), self_id.clone()));
    }
    // Open every peer tunnel from the start (ADR-0067 §2), on every OS.
    tokio::spawn(hold_tunnels_at_start(peers_dir, self_id, port));
}

/// Open a keepalive for every announced peer that can be nudged, skipping this
/// daemon's own descriptor (a WSL daemon writes into the same store). Failures
/// are logged per peer: a distro that no longer exists must not keep the others
/// asleep, and none of this may abort a listener that is already serving.
async fn wake_fleet_at_start(peers_dir: PathBuf, self_id: Option<String>) {
    let (descriptors, _rejects) = peer::read_peer_store(peers_dir).await;
    for d in descriptors {
        if self_id.as_deref() == Some(d.daemon_id.as_str()) {
            continue;
        }
        let Some(spec) = d.nudge.clone() else {
            continue;
        };
        if let Err(e) = crate::routes::hold_awake(spec).await {
            tracing::warn!(
                peer = %d.environment,
                error = %e,
                "could not hold the peer's distro awake"
            );
        }
    }
}

/// Ensure every peer tunnel once at start, off the reactor. Nothing here may
/// abort a listener that is already serving.
async fn hold_tunnels_at_start(peers_dir: PathBuf, self_id: Option<String>, port: u16) {
    let (descriptors, _rejects) = peer::read_peer_store(peers_dir).await;
    let held = tokio::task::spawn_blocking(move || {
        let me = peer::client::SelfRef {
            port,
            daemon_id: self_id.as_deref().unwrap_or(""),
        };
        peer::tunnel::tunnels().ensure_all(&descriptors, me);
    })
    .await;
    if let Err(e) = held {
        tracing::warn!(error = %e, "opening the peer tunnels did not complete");
    }
}

/// Build the descriptor this daemon announces. Pure: every input is a
/// parameter, so the three branches a live boot cannot easily exercise
/// (un-baptized, no WSL, an existing vs a freshly minted token) are unit-tested.
///
/// The announced address is always loopback: the peer transport is loopback-only
/// (ADR-0052 §2), and WSL2's `localhostForwarding` relay is what carries it
/// across the boundary.
pub(crate) fn announced_descriptor(
    id: &identity::Identity,
    port: u16,
    wsl_distro: Option<&str>,
    token: String,
) -> peer::PeerDescriptor {
    peer::PeerDescriptor {
        daemon_id: id.id.to_string(),
        name: id.name.clone(),
        avatar: id.avatar.clone(),
        address: Ipv4Addr::LOCALHOST.to_string(),
        port,
        environment: peer::environment_label(
            wsl_distro,
            std::env::consts::OS,
            wsl_distro
                .is_none()
                .then(peer::system_release)
                .flatten()
                .as_deref(),
        ),
        os: std::env::consts::OS.to_string(),
        token,
        protocol_version: peer::PEER_PROTOCOL_VERSION,
        tunnel: None,
        // Only a daemon inside WSL can be woken by `wsl.exe`, so only it
        // advertises how.
        nudge: wsl_distro.map(|distro| peer::NudgeSpec {
            distro: distro.to_string(),
            unit: autostart::UNIT_NAME.to_string(),
        }),
    }
}

/// Write this daemon's peer descriptor into every store in `stores` (ADR-0052
/// §3). Each daemon announces its OWN token — there is no shared secret, so
/// rotating one daemon's token revokes exactly that one peer.
///
/// `token` is the ALREADY-RESOLVED effective token; one is minted here only when
/// there is none, so announcing never displaces an env credential nor changes
/// the bind policy. An un-baptized daemon has no identity to announce and is
/// skipped with a warning naming the command that fixes it.
pub(crate) fn announce_peer(
    stores: &[PathBuf],
    id: Option<&identity::Identity>,
    addr: SocketAddr,
    token: Option<String>,
) {
    // A daemon that does not listen on loopback cannot be reached by a peer, and
    // announcing `127.0.0.1` for it would produce a descriptor that dials a port
    // nothing answers — an `Unreachable` that never says the descriptor is wrong.
    // Peers reach each other over loopback only: ADR-0052 §2.
    if !addr.ip().is_loopback() && !addr.ip().is_unspecified() {
        tracing::warn!(
            bind = %addr.ip(),
            "--peer-store is ignored: this daemon does not listen on 127.0.0.1, and \
             other daemons can reach it only there"
        );
        return;
    }
    let Some(id) = id else {
        tracing::warn!(
            "--peer-store is ignored: this daemon has no name yet — run \
             `ralphy daemon setup`, then restart the daemon"
        );
        return;
    };
    let token = match token {
        Some(token) if !token.is_empty() => token,
        // No credential yet: mint the daemon's one and only token now. On a
        // loopback bind this changes no policy (`AuthPolicy::for_bind`), and the
        // peer needs SOMETHING to present.
        _ => match auth::token_path().and_then(|p| auth::ensure_token_at(&p)) {
            Ok((token, _minted)) => token,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "--peer-store is ignored: could not read this daemon's access token"
                );
                return;
            }
        },
    };
    let distro = std::env::var("WSL_DISTRO_NAME")
        .ok()
        .filter(|d| !d.is_empty());
    let descriptor = announced_descriptor(id, addr.port(), distro.as_deref(), token);
    for store in stores {
        match peer::write_descriptor(store, &descriptor) {
            Ok(path) => tracing::info!(path = %path.display(), "announced this daemon as a peer"),
            Err(e) => {
                tracing::warn!(
                    store = %store.display(),
                    error = %e,
                    "could not announce into this peer store"
                );
            }
        }
    }
}
