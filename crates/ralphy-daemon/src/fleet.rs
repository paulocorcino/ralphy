//! The federated repo view (docs/adr/0052 §5): every repo the local fleet knows
//! about, from this daemon and from each peer, in one list.
//!
//! The key is `<daemon_id>/<slug>`, NOT the slug. Two daemons may each have
//! `owner/repo` checked out at different paths in different environments; those
//! are two rows, and a slug-keyed map would silently let one overwrite the other.
//!
//! An unreachable peer is MARKED, never dropped: its last-known rows stay listed
//! with `peer_state` naming why they cannot be acted on. Removing them would make
//! a stopped WSL distro look like a machine that never had any repos.

use serde::Serialize;

use crate::peer::client::PeerStatus;
use crate::peer::PeerDescriptor;
use crate::registry::{RegistryStore, RepoEntry};

pub mod route;

pub use route::{peer_unreachable, route, Route};

#[cfg(test)]
mod tests;

/// The `peer_state` of a row this daemon owns. Not a [`PeerStatus`]: the local
/// daemon is not its own peer, and calling it "reachable" would invite the UI to
/// treat it like one.
pub const LOCAL_STATE: &str = "local";

/// One repo in the federated view, attributed to the daemon that registered it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FederatedRepo {
    /// `<daemon_id>/<slug>` — the identity of a row in the fleet.
    pub key: String,
    pub daemon_id: String,
    pub daemon_name: String,
    pub environment: String,
    /// [`LOCAL_STATE`], or the peer's liveness state.
    pub peer_state: String,
    pub slug: String,
    pub path: String,
    /// Whether the path resolves to a directory. Read-time for a LOCAL row; for a
    /// peer row it is the peer's own answer, which is `false` while the peer is
    /// not reachable — this daemon never stats another environment's filesystem.
    pub reachable: bool,
    pub branch: Option<String>,
    pub local: bool,
}

/// Fold the local registry and every peer's into one federated list, sorted by
/// `(environment, daemon_id, slug)` so the order is stable across the arbitrary
/// order peers arrive in.
///
/// `local` is `(daemon_id, daemon_name, environment, store)`. Each peer entry is
/// `(descriptor, status, last-known store)`; a peer with no store contributes
/// zero rows but is never itself dropped from the caller's peer list.
///
/// Deterministic given the filesystem: the only reads are the local rows'
/// `reachable`/`branch`, the same read-time derivation `/api/repos` does. A peer
/// row is pure data — its path lives in another environment.
pub fn aggregate(
    local: (&str, &str, &str, &RegistryStore),
    peers: &[(&PeerDescriptor, &PeerStatus, Option<&RegistryStore>)],
) -> Vec<FederatedRepo> {
    let (local_id, local_name, local_env, local_store) = local;
    let mut rows: Vec<FederatedRepo> = local_store
        .repos
        .iter()
        .map(|(slug, entry)| FederatedRepo {
            key: format!("{local_id}/{slug}"),
            daemon_id: local_id.to_string(),
            daemon_name: local_name.to_string(),
            environment: local_env.to_string(),
            peer_state: LOCAL_STATE.to_string(),
            slug: slug.clone(),
            path: entry.path.clone(),
            reachable: entry.reachable(),
            branch: entry.head_branch(),
            local: true,
        })
        .collect();

    for (d, status, store) in peers {
        let Some(store) = store else { continue };
        let reachable = matches!(status, PeerStatus::Reachable);
        rows.extend(store.repos.iter().map(|(slug, entry)| FederatedRepo {
            key: format!("{}/{}", d.daemon_id, slug),
            daemon_id: d.daemon_id.clone(),
            daemon_name: d.name.clone(),
            environment: d.environment.clone(),
            peer_state: status.state().to_string(),
            slug: slug.clone(),
            path: entry.path.clone(),
            reachable,
            branch: None,
            local: false,
        }));
    }

    rows.sort_by(|a, b| {
        (&a.environment, &a.daemon_id, &a.slug).cmp(&(&b.environment, &b.daemon_id, &b.slug))
    });
    rows
}

/// Build a registry store from a peer's `/api/repos` answer — the JSON array of
/// `{slug, path, …}` objects. Unknown fields are ignored, so a newer peer's
/// richer row still folds (the peer protocol version is the compatibility gate,
/// not the shape of this payload).
pub fn store_from_repos_json(body: &[u8]) -> Option<RegistryStore> {
    #[derive(serde::Deserialize)]
    struct Row {
        slug: String,
        path: String,
    }
    let rows: Vec<Row> = serde_json::from_slice(body).ok()?;
    let mut store = RegistryStore::default();
    for row in rows {
        store.repos.insert(row.slug, RepoEntry { path: row.path });
    }
    Some(store)
}
