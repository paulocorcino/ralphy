//! Pure ownership routing for repo refs.

use crate::peer::PeerDescriptor;

#[cfg(test)]
mod tests;

/// The daemon that owns a repo ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route<'a> {
    Local {
        slug: &'a str,
    },
    Peer {
        peer: &'a PeerDescriptor,
        slug: &'a str,
    },
    UnknownDaemon {
        daemon_id: &'a str,
    },
}

/// Classify a repo ref without performing I/O.
pub fn route<'a>(repo_ref: &'a str, local_id: &str, peers: &'a [PeerDescriptor]) -> Route<'a> {
    let Some((head, tail)) = repo_ref.split_once('/') else {
        return Route::Local { slug: repo_ref };
    };
    if head.parse::<ulid::Ulid>().is_err() {
        return Route::Local { slug: repo_ref };
    }
    if head == local_id {
        return Route::Local { slug: tail };
    }
    match peers.iter().find(|peer| peer.daemon_id == head) {
        Some(peer) => Route::Peer { peer, slug: tail },
        None => Route::UnknownDaemon { daemon_id: head },
    }
}

/// Render a failed peer operation with the peer's environment attached.
pub fn peer_unreachable(peer: &PeerDescriptor, why: &str) -> String {
    crate::peer::client::PeerStatus::Unreachable {
        why: why.to_string(),
    }
    .diagnosis(&peer.environment)
}
