//! `/api/sessions` and `/api/sessions/close`: the session list across the
//! fleet and the close verb, relayed to the owning peer when needed.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;

use super::{session_ws_upgrade, RouterShared, SessionHost, SessionQuery};
use crate::routes::read_peer_store;
use crate::{agent_state, fleet, identity, peer, session};

/// Query for `POST /api/sessions/close`: which session to end.
#[derive(serde::Deserialize)]
pub(crate) struct CloseQuery {
    pub(crate) id: u64,
    pub(crate) repo: Option<String>,
}

#[derive(serde::Deserialize)]
pub(crate) struct SessionsQuery {
    pub(crate) local: Option<u32>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct HostedSessionInfo {
    pub(crate) id: session::SessionId,
    pub(crate) repo: String,
    pub(crate) agent: String,
    pub(crate) kind: String,
    pub(crate) started_at: u64,
    pub(crate) daemon_id: String,
    pub(crate) environment: String,
    /// The name the child answers to in its vendor's own session roster, when it
    /// takes one. `serde(default)` is LOAD BEARING: this type is deserialized
    /// from a PEER's `/api/sessions` body, and a peer on an older build sends no
    /// such field — without the default the whole fleet listing would fail to
    /// parse rather than lose one attribute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    /// The worktree the console lives in (ADR-0063 §3). `serde(default)` is
    /// load-bearing for the same reason as `name`'s: an older peer sends none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) checkout: Option<String>,
    /// The window record the session serves (ADR-0050 amendment 2026-10-04).
    /// `serde(default)` for the same reason as `name`'s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) record: Option<String>,
    /// The agent's hook-reported state (ADR-0059 §5), already rendered with
    /// the staleness rule by the daemon that owns the PTY. `serde(default)`
    /// for the same reason as the two above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent_state: Option<agent_state::AgentState>,
}

pub(crate) fn hosted_session(
    info: session::SessionInfo,
    daemon_id: &str,
    environment: &str,
) -> HostedSessionInfo {
    let effective_environment = info.environment.unwrap_or_else(|| environment.to_string());
    HostedSessionInfo {
        id: info.id,
        repo: info.repo,
        agent: info.agent,
        kind: info.kind,
        started_at: info.started_at,
        daemon_id: daemon_id.to_string(),
        environment: effective_environment,
        name: info.name,
        checkout: info.checkout,
        record: info.record,
        agent_state: info.agent_state,
    }
}

/// `GET /api/sessions`: local and peer-owned live sessions. Peer numeric IDs
/// remain authoritative; the composite repo ref supplies their collision-safe
/// owner key for reattach and close.
pub(crate) async fn sessions_route(
    sessions: Arc<session::SessionManager>,
    peers_dir: PathBuf,
    identity: Option<identity::Identity>,
    environment: String,
    local_only: bool,
) -> Response {
    let daemon_id = identity
        .as_ref()
        .map(|identity| identity.id.to_string())
        .unwrap_or_default();
    let mut rows: Vec<HostedSessionInfo> = sessions
        .list()
        .into_iter()
        .map(|info| hosted_session(info, &daemon_id, &environment))
        .collect();
    if local_only {
        return Json(rows).into_response();
    }
    let (peers, _) = read_peer_store(peers_dir).await;
    let replies = futures_util::future::join_all(peers.into_iter().map(|peer| async move {
        let reply = peer::client::get(&peer, "/api/sessions?local=1").await;
        (peer, reply)
    }))
    .await;
    // A peer missing from the list is not a peer with no sessions: the page
    // must not relaunch, adopt or forget its consoles on this read (ADR-0050
    // amendment 2026-10-04, "a list that did not hear from a peer").
    let mut unanswered = Vec::new();
    for (peer, reply) in replies {
        let Ok((200, body)) = reply else {
            unanswered.push(peer.daemon_id);
            continue;
        };
        let Ok(peer_rows) = serde_json::from_slice::<Vec<HostedSessionInfo>>(&body) else {
            tracing::warn!(
                daemon_id = %peer.daemon_id,
                "peer returned an invalid session list"
            );
            unanswered.push(peer.daemon_id);
            continue;
        };
        rows.extend(peer_rows.into_iter().map(|mut row| {
            row.repo = format!("{}/{}", peer.daemon_id, row.repo);
            row
        }));
    }
    let mut response = Json(rows).into_response();
    // A header, not a body field: the body stays the array every reader and
    // the peer-to-peer `?local=1` form already parse.
    if !unanswered.is_empty() {
        match unanswered.join(",").parse() {
            Ok(value) => {
                response.headers_mut().insert(UNANSWERED_HEADER, value);
            }
            Err(e) => tracing::warn!(error = %e, "could not name the peers that did not answer"),
        }
    }
    response
}

/// The response header of `GET /api/sessions` that names, comma-separated,
/// the peers whose sessions the list does not hold because they did not
/// answer. Absent when every peer answered.
pub(crate) const UNANSWERED_HEADER: &str = "x-ralphy-unanswered";

/// `POST /api/sessions/close?id=<id>[&repo=<repo-ref>]`: end a local session or
/// route a peer-owned numeric id using its composite repo ref.
pub(crate) async fn close_session_route(
    Query(q): Query<CloseQuery>,
    sessions: Arc<session::SessionManager>,
    peers_dir: PathBuf,
    identity: Option<identity::Identity>,
) -> Response {
    if let Some(repo_ref) = q.repo.as_deref() {
        if sessions.get(q.id).is_some_and(|info| info.repo == repo_ref) {
            return if sessions.close(q.id) {
                Json(serde_json::json!({ "closed": true })).into_response()
            } else {
                (StatusCode::NOT_FOUND, "unknown session").into_response()
            };
        }
        let daemon_id = identity
            .as_ref()
            .map(|identity| identity.id.to_string())
            .unwrap_or_default();
        let (peers, rejects) = read_peer_store(peers_dir).await;
        match fleet::route(repo_ref, &daemon_id, &peers) {
            fleet::route::Route::Peer { peer, .. } => {
                let path = format!("/api/sessions/close?id={}", q.id);
                return match peer::client::post_json(peer, &path, &serde_json::json!({})).await {
                    Ok((200, body)) => (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "application/json")],
                        body,
                    )
                        .into_response(),
                    Ok((404, _)) => (StatusCode::NOT_FOUND, "unknown session").into_response(),
                    Ok((status, body)) => (
                        StatusCode::BAD_GATEWAY,
                        format!(
                            "peer {} answered HTTP {status}: {}",
                            peer.environment,
                            String::from_utf8_lossy(&body)
                        ),
                    )
                        .into_response(),
                    Err(error) => (
                        StatusCode::BAD_GATEWAY,
                        fleet::route::peer_unreachable(peer, &format!("{error:#}")),
                    )
                        .into_response(),
                };
            }
            fleet::route::Route::UnknownDaemon { daemon_id } => {
                return (
                    StatusCode::BAD_GATEWAY,
                    fleet::unknown_daemon(daemon_id, &rejects),
                )
                    .into_response();
            }
            fleet::route::Route::Local { .. } => {}
        }
    }
    if sessions.close(q.id) {
        Json(serde_json::json!({ "closed": true })).into_response()
    } else {
        (StatusCode::NOT_FOUND, "unknown session").into_response()
    }
}

/// `/ws/session`, `/api/sessions` and `/api/sessions/close`.
pub(crate) fn session_routes(s: &RouterShared) -> Router {
    let host = SessionHost {
        peers_dir: s.peers_dir.clone(),
        identity: s.identity.clone(),
        environment: s.environment.clone(),
        bound_port: s.bound_port,
    };
    Router::new()
        .route(
            "/ws/session",
            get({
                let sessions = s.sessions.clone();
                let registry = s.registry_path.clone();
                // A live session bridge stops serving on graceful shutdown: it
                // detaches, and never closes the session.
                let shutdown = s.shutdown.clone();
                move |ws: WebSocketUpgrade, q: Query<SessionQuery>| {
                    let sessions = sessions.clone();
                    let registry_path = registry.clone();
                    let shutdown = shutdown.clone();
                    let host = host.clone();
                    async move {
                        session_ws_upgrade(ws, q, sessions, registry_path, host, shutdown).await
                    }
                }
            }),
        )
        .route(
            "/api/sessions",
            get({
                let sessions = s.sessions.clone();
                let peers = s.peers_dir.clone();
                let identity = s.identity.clone();
                let environment = s.environment.clone();
                move |Query(query): Query<SessionsQuery>| {
                    sessions_route(
                        sessions.clone(),
                        peers.clone(),
                        identity.clone(),
                        environment.clone(),
                        query.local == Some(1),
                    )
                }
            }),
        )
        .route(
            "/api/sessions/close",
            post({
                let sessions = s.sessions.clone();
                let peers = s.peers_dir.clone();
                let identity = s.identity.clone();
                move |q: Query<CloseQuery>| {
                    close_session_route(q, sessions.clone(), peers.clone(), identity.clone())
                }
            }),
        )
}
