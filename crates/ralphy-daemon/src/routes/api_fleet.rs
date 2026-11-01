//! `/api/fleet*`: the federated repo list, and the nudge that starts a peer
//! (ADR-0052).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::Query;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;

use super::RouterShared;
use crate::peer::read_peer_store;
use crate::{fleet, identity, peer, registry};

mod nudge;
pub(crate) use nudge::*;

#[cfg(test)]
mod tests;

/// What each peer last showed this daemon, keyed by `daemon_id`. Held for the
/// router's lifetime — same ownership model as `sessions`/`watchers`, so the
/// public `router` signature holds.
pub(crate) type PeerRepoCache =
    Arc<std::sync::Mutex<std::collections::HashMap<String, PeerMemory>>>;

/// The last repo list a peer served and the last environment label its
/// handshake gave. Each is kept until a newer answer replaces it.
#[derive(Default)]
pub(crate) struct PeerMemory {
    pub(crate) repos: Option<fleet::PeerRepoStore>,
    pub(crate) environment: Option<String>,
}

/// Every former key the peers last served, mapped to the key it now has
/// (`fleet::peer_former_slug_map`, over every peer in the cache). The guard is
/// taken and dropped here, with no `.await` between.
pub(crate) fn peer_slug_aliases(
    cache: &PeerRepoCache,
) -> std::collections::BTreeMap<String, String> {
    let cache = match cache.lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    cache
        .iter()
        .filter_map(|(daemon_id, memory)| {
            memory
                .repos
                .as_ref()
                .map(|store| fleet::peer_former_slug_map(daemon_id, store))
        })
        .flatten()
        .collect()
}

/// What one probe found: the peer's state, the label its handshake gave, and
/// the repos it served.
type Probed = (
    peer::client::PeerStatus,
    Option<String>,
    Option<fleet::PeerRepoStore>,
);

/// One peer as the workbench sees it: who it is, where it runs, and what this
/// daemon just observed about it. `state` is the grouping key; `diagnosis` is
/// the sentence shown when that state is not `reachable`.
#[derive(serde::Serialize)]
pub(crate) struct PeerView {
    pub(crate) daemon_id: String,
    pub(crate) name: String,
    pub(crate) avatar: String,
    pub(crate) environment: String,
    /// The peer's OS family, for the icon; empty when its descriptor predates it.
    pub(crate) os: String,
    pub(crate) state: String,
    pub(crate) diagnosis: String,
    /// Whether this peer advertised how to wake it (a WSL unit).
    pub(crate) nudgeable: bool,
    /// Whether this daemon reaches the peer through an `ssh` tunnel it holds.
    pub(crate) tunnel: bool,
    /// The SSH destination and key file of that tunnel, so the Hosts dialog
    /// can fill its form to edit the host (ADR-0067, amendment "the Hosts
    /// dialog", H3). What the descriptor holds; never a secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) destination: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) identity_file: Option<String>,
}

/// `GET /api/fleet`: the federated repo view (ADR-0052 §5) — every peer this
/// daemon can see plus every repo the fleet knows, as
/// `{ peers: [...], repos: [...] }`.
///
/// Reads the peer store FRESH and probes every peer on EVERY request, holding no
/// background state: same contract as `/api/repos`. A cached liveness table would
/// contradict "a descriptor is a claim, not a fact" — the answer is what is true
/// now, not what was true when a poller last ran.
///
/// `/api/repos` is deliberately untouched: this route is additive, so nothing
/// pinned to the local list changes shape.
pub(crate) async fn fleet_route(
    registry_path: PathBuf,
    peers_dir: PathBuf,
    identity: Option<identity::Identity>,
    environment: String,
    repo_cache: PeerRepoCache,
    bound_port: u16,
) -> Response {
    let (mut descriptors, rejects) = read_peer_store(peers_dir.clone()).await;
    let probe_id = identity
        .as_ref()
        .map(|identity| identity.id.to_string())
        .unwrap_or_default();

    let probed = probe_all(&descriptors, &probe_id, bound_port).await;
    let recalled = recall_repos(&repo_cache, &mut descriptors, &probed);

    let fallback = probe_did_not_complete();
    let (mut peer_views, aggregate_input) = peer_rows(&descriptors, &probed, &recalled, &fallback);
    // A rejected record is DEGRADED, never dropped: the operator has a file on
    // disk that is not doing what they think it is, and only this list says so.
    peer_views.extend(rejects.iter().map(reject_view));

    let mut repos = local_rows(&registry_path, identity.as_ref(), &environment).await;
    repos.extend(fleet::aggregate(
        ("", "", &environment, &registry::RegistryStore::default()),
        &aggregate_input,
    ));
    // The order `fleet::aggregate` sorts by.
    repos.sort_by(|a, b| {
        (&a.environment, &a.daemon_id, &a.slug).cmp(&(&b.environment, &b.daemon_id, &b.slug))
    });
    Json(serde_json::json!({ "peers": peer_views, "repos": repos })).into_response()
}

/// Probe every peer, each in its own task; the slot of a peer whose task
/// failed stays `None`.
async fn probe_all(
    descriptors: &[peer::PeerDescriptor],
    probe_id: &str,
    bound_port: u16,
) -> Vec<Option<Probed>> {
    // Probe every peer CONCURRENTLY: with a 2 s per-peer timeout, dialling them
    // one after another would make the page cost the sum of the down ones.
    let mut set = tokio::task::JoinSet::new();
    for (index, d) in descriptors.iter().cloned().enumerate() {
        let probe_id = probe_id.to_string();
        set.spawn(async move { (index, probe_one(d, probe_id, bound_port).await) });
    }
    collect_probes(set, descriptors.len()).await
}

/// One probe: the handshake, then the repos of a peer that answered it. It
/// owns its inputs because it runs as its own task.
async fn probe_one(d: peer::PeerDescriptor, probe_id: String, bound_port: u16) -> Probed {
    let (status, live_environment) = peer::client::probe_hello(
        &d,
        peer::client::SelfRef {
            port: bound_port,
            daemon_id: &probe_id,
        },
    )
    .await;
    // Only a reachable peer is asked for its repos; an unreachable one
    // contributes no rows but is still listed (marked, never removed).
    let store = match status {
        peer::client::PeerStatus::Reachable => read_peer_repos(&d).await,
        peer::client::PeerStatus::Unauthorized { .. }
        | peer::client::PeerStatus::VersionMismatch { .. }
        | peer::client::PeerStatus::Asleep { .. }
        | peer::client::PeerStatus::Unreachable { .. }
        | peer::client::PeerStatus::Refused { .. }
        | peer::client::PeerStatus::TunnelClosed { .. }
        | peer::client::PeerStatus::TunnelSilent { .. } => None,
    };
    (status, live_environment, store)
}

/// The repos a reachable peer serves on `/api/repos`, or `None` when it
/// refuses or serves a body this daemon cannot read.
async fn read_peer_repos(d: &peer::PeerDescriptor) -> Option<fleet::PeerRepoStore> {
    match peer::client::get(d, "/api/repos").await {
        Ok((200, body)) => match fleet::store_from_repos_json(&body) {
            Some(store) => Some(store),
            None => {
                tracing::warn!(peer = %d.environment, "peer served an /api/repos body this daemon cannot read; keeping its last-known repos");
                None
            }
        },
        Ok((code, _)) => {
            tracing::warn!(peer = %d.environment, %code, "peer answered the handshake but refused /api/repos; keeping its last-known repos");
            None
        }
        Err(e) => {
            tracing::warn!(peer = %d.environment, error = %format!("{e:#}"), "could not read a peer's repos; keeping its last-known ones");
            None
        }
    }
}

/// Wait for every probe task and put each result in its peer's slot.
async fn collect_probes(
    mut set: tokio::task::JoinSet<(usize, Probed)>,
    len: usize,
) -> Vec<Option<Probed>> {
    let mut probed: Vec<Option<Probed>> = (0..len).map(|_| None).collect();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((index, found)) => probed[index] = Some(found),
            Err(e) => tracing::warn!(error = %e, "a peer probe task failed"),
        }
    }
    probed
}

/// Remember what each peer just served, and recall it for the ones that could
/// not be asked. Sync on purpose: the guard is a `std::sync::Mutex`.
fn recall_repos(
    repo_cache: &PeerRepoCache,
    descriptors: &mut [peer::PeerDescriptor],
    probed: &[Option<Probed>],
) -> Vec<Option<fleet::PeerRepoStore>> {
    // INVARIANT: the guard is taken and dropped inside this function — it is a
    // `std::sync::Mutex` and there is no `.await` here.
    //
    // The descriptor's label is what the peer said when it was paired; its
    // handshake is what it says now, and the last handshake is what it said
    // before it stopped answering (ADR-0067, amendment "the header in
    // capitals, the label from the handshake"). The file on disk is left as
    // `host add` wrote it.
    let mut cache = match repo_cache.lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    descriptors
        .iter_mut()
        .zip(probed.iter())
        .map(|(d, slot)| {
            let memory = cache.entry(d.daemon_id.clone()).or_default();
            memory.remember(slot.as_ref());
            if let Some(environment) = &memory.environment {
                d.environment = environment.clone();
            }
            memory.repos.clone()
        })
        .collect()
}

impl PeerMemory {
    /// Keep the label and the repos a probe just found; what it did not find
    /// keeps its last value.
    fn remember(&mut self, probed: Option<&Probed>) {
        if let Some((_, live, fresh)) = probed {
            if live.is_some() {
                self.environment = live.clone();
            }
            if fresh.is_some() {
                self.repos = fresh.clone();
            }
        }
    }
}

/// The state of a peer whose probe task itself failed.
fn probe_did_not_complete() -> peer::client::PeerStatus {
    peer::client::PeerStatus::Unreachable {
        why: "the probe did not complete".to_string(),
    }
}

/// One peer as `fleet::aggregate` reads it.
type AggregateInput<'a> = (
    &'a peer::PeerDescriptor,
    &'a peer::client::PeerStatus,
    Option<&'a fleet::PeerRepoStore>,
);

/// The row of every peer, and what `fleet::aggregate` needs of it.
fn peer_rows<'a>(
    descriptors: &'a [peer::PeerDescriptor],
    probed: &'a [Option<Probed>],
    recalled: &'a [Option<fleet::PeerRepoStore>],
    fallback: &'a peer::client::PeerStatus,
) -> (Vec<PeerView>, Vec<AggregateInput<'a>>) {
    let mut peer_views: Vec<PeerView> = Vec::with_capacity(descriptors.len());
    let mut aggregate_input: Vec<AggregateInput<'a>> = Vec::with_capacity(descriptors.len());
    // A probe whose task itself failed is reported as unreachable rather than
    // dropped — the operator must see the peer, not a silently shorter list.
    for ((d, slot), store) in descriptors.iter().zip(probed.iter()).zip(recalled.iter()) {
        let status = match slot {
            Some((status, _, _)) => status,
            None => fallback,
        };
        peer_views.push(peer_view(d, status));
        aggregate_input.push((d, status, store.as_ref()));
    }
    (peer_views, aggregate_input)
}

/// The row of one peer, from its descriptor and the state just observed.
fn peer_view(d: &peer::PeerDescriptor, status: &peer::client::PeerStatus) -> PeerView {
    PeerView {
        daemon_id: d.daemon_id.clone(),
        name: d.name.clone(),
        avatar: d.avatar.clone(),
        environment: d.environment.clone(),
        os: d.os.clone(),
        state: status.state().to_string(),
        diagnosis: status.diagnosis(&d.environment),
        nudgeable: d.nudge.is_some(),
        tunnel: d.tunnel.is_some(),
        destination: d.tunnel.as_ref().map(|t| t.destination.clone()),
        identity_file: d.tunnel.as_ref().and_then(|t| t.identity_file.clone()),
    }
}

/// The row of a peer record this daemon could not read.
fn reject_view(reject: &peer::PeerReject) -> PeerView {
    PeerView {
        // A synthetic id keyed on the FILE: the browser groups by
        // `daemon_id`, so a shared empty string would collapse two bad
        // descriptors into one row and lose the first one's diagnosis.
        daemon_id: format!("malformed:{}", reject.file()),
        name: reject.file().to_string(),
        avatar: "❔".to_string(),
        environment: "unknown".to_string(),
        os: String::new(),
        state: "malformed".to_string(),
        diagnosis: reject.why(),
        nudgeable: false,
        tunnel: false,
        destination: None,
        identity_file: None,
    }
}

/// The rows of this daemon's own repos; none when they cannot be built.
async fn local_rows(
    registry_path: &Path,
    identity: Option<&identity::Identity>,
    environment: &str,
) -> Vec<fleet::FederatedRepo> {
    let local_store = match registry::load_from(registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry; federating an empty local list");
            registry::RegistryStore::default()
        }
    };
    let local_id = identity.map(|i| i.id.to_string()).unwrap_or_default();
    let local_name = identity.map(|i| i.name.clone()).unwrap_or_default();
    // A local row reads its branch through git, so the local rows are built on
    // the blocking pool; the peer rows need no git.
    let environment = environment.to_string();
    let local_rows = tokio::task::spawn_blocking(move || {
        fleet::aggregate((&local_id, &local_name, &environment, &local_store), &[])
    })
    .await;
    match local_rows {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "the local repo rows did not complete; federating none");
            Vec::new()
        }
    }
}

/// `/api/fleet*`: the federated repo list and the nudge.
pub(crate) fn fleet_routes(s: &RouterShared) -> Router {
    let bound_port = s.bound_port;
    Router::new()
        .route(
            "/api/fleet",
            get({
                let registry = s.registry_path.clone();
                let peers = s.peers_dir.clone();
                let id = s.identity.clone();
                let env = s.environment.clone();
                let cache = s.peer_repo_cache.clone();
                move || {
                    fleet_route(
                        registry.clone(),
                        peers.clone(),
                        id.clone(),
                        env.clone(),
                        cache.clone(),
                        bound_port,
                    )
                }
            }),
        )
        .route(
            "/api/fleet/nudge",
            post({
                let peers = s.peers_dir.clone();
                // The nudge waits for the peer to answer, so it probes — and a
                // probe needs to know who this daemon is to refuse dialling itself.
                let daemon_id = s.daemon_id.clone();
                move |q: Query<NudgeQuery>| {
                    fleet_nudge_route(peers.clone(), daemon_id.clone(), bound_port, q.0.daemon_id)
                }
            }),
        )
}
