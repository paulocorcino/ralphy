//! `/api/fleet*` and `/api/peer/*`: the federated repo list, the nudge, and
//! the endpoints a PEER daemon calls on this one (ADR-0052).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::execute_oneshot;
use super::read_peer_store;
use crate::{dispatch, fleet, identity, peer, protocol, registry, session, watch};

/// The last repo list each peer served, keyed by `daemon_id`. Held for the
/// router's lifetime — same ownership model as `sessions`/`watchers`, so the
/// public `router` signature holds.
pub(crate) type PeerRepoCache =
    Arc<std::sync::Mutex<std::collections::HashMap<String, fleet::PeerRepoStore>>>;

/// One peer as the workbench sees it: who it is, where it runs, and what this
/// daemon just observed about it. `state` is the grouping key; `diagnosis` is
/// the sentence shown when that state is not `reachable`.
#[derive(serde::Serialize)]
pub(crate) struct PeerView {
    pub(crate) daemon_id: String,
    pub(crate) name: String,
    pub(crate) avatar: String,
    pub(crate) environment: String,
    pub(crate) state: String,
    pub(crate) diagnosis: String,
    /// Whether this peer advertised how to wake it (a WSL unit).
    pub(crate) nudgeable: bool,
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
    let (descriptors, rejects) = read_peer_store(peers_dir.clone()).await;
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
            let status = peer::client::probe(
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
            (index, status, store)
        });
    }
    let mut probed: Vec<Option<(peer::client::PeerStatus, Option<fleet::PeerRepoStore>)>> =
        (0..descriptors.len()).map(|_| None).collect();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((index, status, store)) => probed[index] = Some((status, store)),
            Err(e) => tracing::warn!(error = %e, "a peer probe task failed"),
        }
    }

    // Remember what each peer just served, and recall it for the ones that could
    // not be asked. INVARIANT: the guard is taken and dropped inside this block —
    // it is a `std::sync::Mutex` and there is no `.await` between these lines.
    let recalled: Vec<Option<fleet::PeerRepoStore>> = {
        let mut cache = match repo_cache.lock() {
            Ok(cache) => cache,
            Err(poisoned) => poisoned.into_inner(),
        };
        descriptors
            .iter()
            .zip(probed.iter())
            .map(
                |(d, slot)| match slot.as_ref().and_then(|(_, s)| s.as_ref()) {
                    Some(fresh) => {
                        cache.insert(d.daemon_id.clone(), fresh.clone());
                        Some(fresh.clone())
                    }
                    None => cache.get(&d.daemon_id).cloned(),
                },
            )
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
            Some((status, _)) => status,
            None => &fallback,
        };
        let store = store.as_ref();
        peer_views.push(PeerView {
            daemon_id: d.daemon_id.clone(),
            name: d.name.clone(),
            avatar: d.avatar.clone(),
            environment: d.environment.clone(),
            state: status.state().to_string(),
            diagnosis: status.diagnosis(&d.environment),
            nudgeable: d.nudge.is_some(),
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
            state: "malformed".to_string(),
            diagnosis: reject.why(),
            nudgeable: false,
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
    let repos = fleet::aggregate(
        (&local_id, &local_name, &environment, &local_store),
        &aggregate_input,
    );
    Json(serde_json::json!({ "peers": peer_views, "repos": repos })).into_response()
}

/// The `daemon_id` a nudge is aimed at.
#[derive(serde::Deserialize)]
pub(crate) struct NudgeQuery {
    pub(crate) daemon_id: String,
}

/// `POST /api/fleet/nudge?daemon_id=<id>`: ask the OS to start a peer that is not
/// answering (ADR-0052 §4). Fire-and-forget — the daemon spawns and never
/// parents, holds, or signals what it started. The keepalive comes first: it is
/// what boots a stopped distro AND what keeps it booted; the unit start after it
/// covers the other half of `unreachable`, a running distro whose unit is down.
pub(crate) async fn fleet_nudge_route(
    peers_dir: PathBuf,
    self_daemon_id: Option<String>,
    bound_port: u16,
    daemon_id: String,
) -> Response {
    let (descriptors, _rejects) = read_peer_store(peers_dir.clone()).await;
    let Some(d) = descriptors.into_iter().find(|d| d.daemon_id == daemon_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("No peer is announced as {daemon_id}.") })),
        )
            .into_response();
    };
    let Some(spec) = d.nudge.as_ref() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "Peer {} announced no way to wake it. A WSL peer needs `loginctl enable-linger` \
                     and a `ralphy-daemon.service` user unit.",
                    d.environment
                )
            })),
        )
            .into_response();
    };
    if let Err(e) = hold_awake(spec.clone()).await {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("{e:#}") })),
        )
            .into_response();
    }
    let argv = peer::nudge::nudge_argv(spec);
    if let Err(e) = peer::nudge::spawn_detached(&argv) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("{e:#}") })),
        )
            .into_response();
    }
    let (ready, waited, last) = nudge_await_ready(
        &d,
        self_daemon_id.as_deref(),
        bound_port,
        peer::nudge::READY_DEADLINE,
    )
    .await;
    // 200 either way: the nudge itself succeeded, and whether the peer came back
    // is an OBSERVATION this reports, not a failure of the request. `ready` is the
    // field a caller acts on — the old `nudged` is kept so an older workbench
    // keeps working.
    Json(serde_json::json!({
        "nudged": true,
        "ready": ready,
        "waited_ms": waited.as_millis() as u64,
        "state": last.state(),
        "diagnosis": last.diagnosis(&d.environment),
    }))
    .into_response()
}

/// Hold `spec.distro` open from the reactor: the keepalive spawns a process, so
/// it runs on the blocking pool. Logs when a handle was actually opened, so a
/// daemon log reads which distros this process is holding.
pub(crate) async fn hold_awake(spec: peer::NudgeSpec) -> anyhow::Result<()> {
    let distro = spec.distro.clone();
    let spawned = tokio::task::spawn_blocking(move || peer::nudge::keepalives().ensure(&spec))
        .await
        .context("the keepalive task did not complete")??;
    if spawned {
        tracing::info!(%distro, "holding the distro awake for its peer daemon (ADR-0052 §4)");
    }
    Ok(())
}

/// Poll a just-nudged peer until it answers the handshake, or until `deadline`.
/// Returns whether it came back, how long that took, and the last status seen.
///
/// Waiting is not supervising (ADR-0052 §4): nothing here parents, holds or
/// signals the process the nudge started — systemd inside the distro owns it. All
/// this does is answer the operator's own question, which used to be answered
/// with "spawned" while the honest answer was "not yet".
///
/// `deadline` is a parameter rather than the constant so a test can pin the
/// give-up path without spending [`peer::nudge::READY_DEADLINE`] on it.
pub(crate) async fn nudge_await_ready(
    d: &peer::PeerDescriptor,
    self_daemon_id: Option<&str>,
    bound_port: u16,
    deadline: Duration,
) -> (bool, Duration, peer::client::PeerStatus) {
    let me = peer::client::SelfRef {
        port: bound_port,
        daemon_id: self_daemon_id.unwrap_or_default(),
    };
    let started = Instant::now();
    loop {
        let status = peer::client::probe(d, me).await;
        if status == peer::client::PeerStatus::Reachable {
            return (true, started.elapsed(), status);
        }
        // A refusal is a verdict about the descriptor, not about a boot in
        // progress: no amount of waiting turns a self-dial or a routable address
        // into a reachable peer, and neither does a version mismatch.
        if matches!(
            status,
            peer::client::PeerStatus::Refused { .. }
                | peer::client::PeerStatus::VersionMismatch { .. }
        ) {
            return (false, started.elapsed(), status);
        }
        if started.elapsed() >= deadline {
            return (false, started.elapsed(), status);
        }
        tokio::time::sleep(peer::nudge::READY_POLL).await;
    }
}

/// `GET /api/peer/hello`: the local fleet's version handshake (ADR-0052 §3) —
/// who this daemon is, which environment it runs in, and which peer protocol it
/// speaks. Cheap and side-effect-free, and deliberately NOT folded into
/// `/api/identity`, whose 404-when-un-baptized contract the browser depends on.
///
/// 404 when un-baptized, matching `/api/identity`: a daemon with no identity has
/// nothing a peer could key on.
pub(crate) async fn peer_hello_route(
    identity: Option<identity::Identity>,
    environment: String,
) -> Response {
    #[derive(serde::Serialize)]
    struct HelloView {
        daemon_id: String,
        name: String,
        avatar: String,
        environment: String,
        protocol_version: u32,
    }
    match identity {
        Some(id) => Json(HelloView {
            daemon_id: id.id.to_string(),
            name: id.name,
            avatar: id.avatar,
            environment,
            protocol_version: peer::PEER_PROTOCOL_VERSION,
        })
        .into_response(),
        None => (StatusCode::NOT_FOUND, "no identity").into_response(),
    }
}

/// Execute a peer command against this daemon's local registry only.
///
/// The repo is a bare slug here. This boundary never routes again, which makes
/// proxy loops unrepresentable.
pub(crate) async fn peer_command_route(
    registry_path: PathBuf,
    daemon_id: Option<String>,
    sessions: Arc<session::SessionManager>,
    Json(cmd): Json<protocol::Command>,
) -> Response {
    let Some(verb) = dispatch::Verb::from_query(&cmd.verb) else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "unknown verb"
        }))
        .into_response();
    };
    if verb.effect_class() == dispatch::EffectClass::Spawn {
        return Json(serde_json::json!({
            "status": "error",
            "message": "a run is not federated yet"
        }))
        .into_response();
    }
    let slug = cmd
        .payload
        .get("repo")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry for a peer command");
            return Json(serde_json::json!({
                "status": "error",
                "message": "repo registry unreadable"
            }))
            .into_response();
        }
    };
    let Some(entry) = store.entry(slug) else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "unknown repo"
        }))
        .into_response();
    };
    let payload = execute_oneshot(
        verb,
        &cmd,
        Path::new(&entry.path),
        daemon_id.as_deref(),
        slug,
        &sessions,
    )
    .await
    .unwrap_or_else(|| {
        serde_json::json!({
            "status": "error",
            "message": "a run is not federated yet"
        })
    });
    Json(payload).into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct PeerTreePoll {
    pub(crate) sub: String,
    pub(crate) repo: String,
    pub(crate) paths: Vec<String>,
    pub(crate) runs: bool,
    pub(crate) timeout_ms: u64,
}

#[derive(serde::Deserialize)]
pub(crate) struct PeerTreeClose {
    pub(crate) sub: String,
}

/// Long-poll one buffered tree subscription against this daemon's local repo.
pub(crate) async fn peer_tree_poll_route(
    registry_path: PathBuf,
    subs: Arc<fleet::watchsub::WatchSubs>,
    Json(mut poll): Json<PeerTreePoll>,
) -> Response {
    // Timed from the TOP, not from the wait: the setup below (a sweep that can
    // tear a watcher down, a registry read) once cost 2.7 s while the log
    // reported a punctual 25 000 ms wait, which is how a caller deadline of 27 s
    // came to look sufficient. What the caller is waiting for is this whole
    // function.
    let started = Instant::now();
    subs.sweep(fleet::watchsub::IDLE_EXPIRY);
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(_) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "repo registry unreadable"
            }))
            .into_response()
        }
    };
    let Some(entry) = store.entry(&poll.repo) else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "unknown repo"
        }))
        .into_response();
    };
    let root = Path::new(&entry.path);
    if poll.runs {
        let runstate = root.join(watch::RUNSTATE_REL);
        if let Err(e) = std::fs::create_dir_all(&runstate) {
            tracing::warn!(path = %runstate.display(), error = %e, "failed to create peer runstate watch directory");
        }
        if !poll.paths.iter().any(|path| path == watch::RUNSTATE_REL) {
            poll.paths.push(watch::RUNSTATE_REL.to_string());
        }
    }
    if let Err(e) = subs.subscribe(&poll.sub, &poll.repo, root, &poll.paths) {
        tracing::warn!(error = %e, "refused a peer tree subscription");
        // NOT `{"dirty": []}`. A refusal dressed as "nothing changed" is answered
        // in milliseconds and re-posted at once; saying it is an error is what
        // puts the caller on its backoff instead.
        return Json(serde_json::json!({
            "status": "error",
            "message": format!("{e:#}")
        }))
        .into_response();
    }
    let (dirty, outcome) = subs
        .wait(
            &poll.sub,
            Duration::from_millis(poll.timeout_ms.min(25_000)),
        )
        .await;
    // The one line that makes a hot poll loop attributable without a packet
    // capture: WHY this long poll came back, and how long it actually held. A
    // healthy poll answers `timeout` after the full window, or `dirty`; anything
    // else answering in milliseconds is the caller spinning (2026-09-01).
    tracing::debug!(
        sub = %poll.sub,
        repo = %poll.repo,
        paths = poll.paths.len(),
        reason = outcome.as_str(),
        dirty = dirty.len(),
        took_ms = started.elapsed().as_millis() as u64,
        "peer tree poll answered"
    );
    let dirty: Vec<serde_json::Value> = dirty
        .into_iter()
        .map(|(repo, path)| serde_json::json!({ "repo": repo, "path": path }))
        .collect();
    Json(serde_json::json!({ "dirty": dirty })).into_response()
}

pub(crate) async fn peer_tree_close_route(
    subs: Arc<fleet::watchsub::WatchSubs>,
    Json(close): Json<PeerTreeClose>,
) -> Response {
    subs.close(&close.sub);
    Json(serde_json::json!({ "closed": true })).into_response()
}
