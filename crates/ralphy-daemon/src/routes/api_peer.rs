//! `/api/peer/*`: the endpoints a PEER daemon calls on this one (ADR-0052).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;

use super::execute_oneshot;
use super::{usage_local_route, RouterShared, UsageQuery};
use crate::{dispatch, fleet, identity, peer, protocol, registry, session, watch};

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
    if verb.is_host() {
        let message = "a host command runs only on the daemon you are connected to";
        return Json(serde_json::json!({ "status": "error", "message": message })).into_response();
    }
    // A registry verb is served against this daemon's own disk; any `daemon`
    // in the payload is ignored, so it never routes again.
    if verb.is_registry() {
        let reply =
            super::ws_command::serve_registry(&cmd, verb, &registry_path, daemon_id.as_deref());
        return Json(reply.await).into_response();
    }
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

/// `/api/peer/*`: the routes another daemon calls on this one.
pub(crate) fn peer_routes(s: &RouterShared) -> Router {
    Router::new()
        .route(
            "/api/peer/hello",
            get({
                let id = s.identity.clone();
                let env = s.environment.clone();
                move || peer_hello_route(id.clone(), env.clone())
            }),
        )
        .route(
            "/api/peer/usage",
            get({
                let dir = s.usage_dir.clone();
                let stores = s.stores.clone();
                let registry = s.registry_path.clone();
                let daemon_id = s.daemon_id.clone();
                move |q: Query<UsageQuery>| {
                    usage_local_route(
                        dir.clone(),
                        stores.clone(),
                        registry.clone(),
                        daemon_id.clone(),
                        q.0.since,
                    )
                }
            }),
        )
        .route(
            "/api/peer/command",
            post({
                let registry = s.registry_path.clone();
                let daemon_id = s.daemon_id.clone();
                let sessions = s.sessions.clone();
                move |body: Json<protocol::Command>| {
                    peer_command_route(registry.clone(), daemon_id.clone(), sessions.clone(), body)
                }
            })
            // axum's 2 MB default refused a forwarded 4 MiB image paste.
            .layer(axum::extract::DefaultBodyLimit::max(
                crate::tree::MAX_COMMAND_BYTES,
            )),
        )
        .route(
            "/api/peer/tree/poll",
            post({
                let registry = s.registry_path.clone();
                let subs = s.peer_watch_subs.clone();
                move |body: Json<PeerTreePoll>| {
                    peer_tree_poll_route(registry.clone(), subs.clone(), body)
                }
            }),
        )
        .route(
            "/api/peer/tree/close",
            post({
                let subs = s.peer_watch_subs.clone();
                move |body: Json<PeerTreeClose>| peer_tree_close_route(subs.clone(), body)
            }),
        )
}
