//! `/api/fleet*`: the federated repo list, and the nudge that starts a peer
//! (ADR-0052).

use std::path::PathBuf;
use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use axum::Json;

use super::read_peer_store;
use crate::{fleet, identity, peer, registry};

mod nudge;
pub(crate) use nudge::*;

/// What each peer last showed this daemon, keyed by `daemon_id`. Held for the
/// router's lifetime — same ownership model as `sessions`/`watchers`, so the
/// public `router` signature holds.
pub(crate) type PeerRepoCache =
    Arc<std::sync::Mutex<std::collections::HashMap<String, PeerMemory>>>;

/// The last repo list a peer served and the last environment label its
/// handshake gave. Each is kept until a newer answer replaces it.
#[derive(Default)]
pub(crate) struct PeerMemory {
    repos: Option<fleet::PeerRepoStore>,
    environment: Option<String>,
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

    // Probe every peer CONCURRENTLY: with a 2 s per-peer timeout, dialling them
    // one after another would make the page cost the sum of the down ones.
    let mut set = tokio::task::JoinSet::new();
    for (index, d) in descriptors.iter().cloned().enumerate() {
        let probe_id = probe_id.clone();
        set.spawn(async move {
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
                peer::client::PeerStatus::Reachable => {
                    match peer::client::get(&d, "/api/repos").await {
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
                _ => None,
            };
            (index, status, live_environment, store)
        });
    }
    let mut probed: Vec<Option<Probed>> = (0..descriptors.len()).map(|_| None).collect();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((index, status, live_environment, store)) => {
                probed[index] = Some((status, live_environment, store));
            }
            Err(e) => tracing::warn!(error = %e, "a peer probe task failed"),
        }
    }

    // Remember what each peer just served, and recall it for the ones that could
    // not be asked. INVARIANT: the guard is taken and dropped inside this block —
    // it is a `std::sync::Mutex` and there is no `.await` between these lines.
    //
    // The descriptor's label is what the peer said when it was paired; its
    // handshake is what it says now, and the last handshake is what it said
    // before it stopped answering (ADR-0067, amendment "the header in
    // capitals, the label from the handshake"). The file on disk is left as
    // `host add` wrote it.
    let recalled: Vec<Option<fleet::PeerRepoStore>> = {
        let mut cache = match repo_cache.lock() {
            Ok(cache) => cache,
            Err(poisoned) => poisoned.into_inner(),
        };
        descriptors
            .iter_mut()
            .zip(probed.iter())
            .map(|(d, slot)| {
                let memory = cache.entry(d.daemon_id.clone()).or_default();
                if let Some((_, live, fresh)) = slot {
                    if live.is_some() {
                        memory.environment = live.clone();
                    }
                    if fresh.is_some() {
                        memory.repos = fresh.clone();
                    }
                }
                if let Some(environment) = &memory.environment {
                    d.environment = environment.clone();
                }
                memory.repos.clone()
            })
            .collect()
    };

    let mut peer_views: Vec<PeerView> = Vec::with_capacity(descriptors.len() + rejects.len());
    let mut aggregate_input: Vec<(
        &peer::PeerDescriptor,
        &peer::client::PeerStatus,
        Option<&fleet::PeerRepoStore>,
    )> = Vec::with_capacity(descriptors.len());
    // A probe whose task itself failed is reported as unreachable rather than
    // dropped — the operator must see the peer, not a silently shorter list.
    let fallback = peer::client::PeerStatus::Unreachable {
        why: "the probe did not complete".to_string(),
    };
    for ((d, slot), store) in descriptors.iter().zip(probed.iter()).zip(recalled.iter()) {
        let status = match slot {
            Some((status, _, _)) => status,
            None => &fallback,
        };
        let store = store.as_ref();
        peer_views.push(PeerView {
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
        });
        aggregate_input.push((d, status, store));
    }
    // A rejected record is DEGRADED, never dropped: the operator has a file on
    // disk that is not doing what they think it is, and only this list says so.
    for reject in &rejects {
        peer_views.push(PeerView {
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
        });
    }

    let local_store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry; federating an empty local list");
            registry::RegistryStore::default()
        }
    };
    let local_id = identity
        .as_ref()
        .map(|i| i.id.to_string())
        .unwrap_or_default();
    let local_name = identity
        .as_ref()
        .map(|i| i.name.clone())
        .unwrap_or_default();
    // A local row reads its branch through git, so the local rows are built on
    // the blocking pool; the peer rows need no git.
    let local_rows = {
        let environment = environment.clone();
        tokio::task::spawn_blocking(move || {
            fleet::aggregate((&local_id, &local_name, &environment, &local_store), &[])
        })
        .await
    };
    let mut repos = match local_rows {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "the local repo rows did not complete; federating none");
            Vec::new()
        }
    };
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
