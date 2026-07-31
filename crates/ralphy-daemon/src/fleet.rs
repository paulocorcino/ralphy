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

use std::collections::BTreeMap;

use crate::peer::client::PeerStatus;
use crate::peer::PeerDescriptor;
use crate::registry::RegistryStore;

pub mod route;
pub mod watchsub;

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

/// One repo as a PEER described it. The owning daemon derives these facts about
/// its own filesystem and serves them on `/api/repos`; this daemon carries them
/// through unread, because §1 forbids it from stat-ing another environment's
/// path to check.
///
/// Keeping `reachable` and `branch` here is the whole point of the type. Folding
/// a peer's answer down to `slug -> path` and re-deriving the rest locally is how
/// a peer repo whose directory is gone came to be listed as healthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRepoRow {
    pub path: String,
    /// The peer's own verdict on its own path — never this daemon's.
    pub reachable: bool,
    pub branch: Option<String>,
}

/// A peer's repos, keyed by slug, as last served by that peer.
pub type PeerRepoStore = BTreeMap<String, PeerRepoRow>;

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
    peers: &[(&PeerDescriptor, &PeerStatus, Option<&PeerRepoStore>)],
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
        // Both must hold. The peer's verdict is about its path; ours is about
        // whether that verdict is current — rows recalled from cache while the
        // peer is down describe a filesystem nobody can reach right now.
        let peer_live = matches!(status, PeerStatus::Reachable);
        rows.extend(store.iter().map(|(slug, row)| FederatedRepo {
            key: format!("{}/{}", d.daemon_id, slug),
            daemon_id: d.daemon_id.clone(),
            daemon_name: d.name.clone(),
            environment: d.environment.clone(),
            peer_state: status.state().to_string(),
            slug: slug.clone(),
            path: row.path.clone(),
            reachable: peer_live && row.reachable,
            branch: row.branch.clone(),
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
pub fn store_from_repos_json(body: &[u8]) -> Option<PeerRepoStore> {
    #[derive(serde::Deserialize)]
    struct Row {
        slug: String,
        path: String,
        reachable: bool,
        #[serde(default)]
        branch: Option<String>,
    }
    let rows: Vec<Row> = serde_json::from_slice(body).ok()?;
    Some(
        rows.into_iter()
            .map(|row| {
                (
                    row.slug,
                    PeerRepoRow {
                        path: row.path,
                        reachable: row.reachable,
                        branch: row.branch,
                    },
                )
            })
            .collect(),
    )
}

/// Locate one repo in a peer-served registry while retaining the owning
/// daemon's reachability verdict. The local daemon must not stat a peer path.
pub fn repo_from_repos_json(
    body: &[u8],
    slug: &str,
) -> serde_json::Result<Option<PeerRepoLocation>> {
    #[derive(serde::Deserialize)]
    struct Row {
        slug: String,
        path: String,
        reachable: bool,
    }
    let rows: Vec<Row> = serde_json::from_slice(body)?;
    Ok(rows.into_iter().find_map(|row| {
        (row.slug == slug).then_some(PeerRepoLocation {
            path: row.path,
            reachable: row.reachable,
        })
    }))
}

pub struct PeerRepoLocation {
    pub path: String,
    pub reachable: bool,
}
