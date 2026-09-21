//! `/api/sessions` and `/api/sessions/close`: the session list across the
//! fleet and the close verb, relayed to the owning peer when needed.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

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
    for (peer, reply) in replies {
        let Ok((200, body)) = reply else {
            continue;
        };
        let Ok(peer_rows) = serde_json::from_slice::<Vec<HostedSessionInfo>>(&body) else {
            tracing::warn!(
                daemon_id = %peer.daemon_id,
                "peer returned an invalid session list"
            );
            continue;
        };
        rows.extend(peer_rows.into_iter().map(|mut row| {
            row.repo = format!("{}/{}", peer.daemon_id, row.repo);
            row
        }));
    }
    Json(rows).into_response()
}

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
                if let Some((environment, theirs)) = rejects
                    .iter()
                    .find_map(|reject| reject.version_mismatch_for(daemon_id))
                {
                    let status = peer::client::PeerStatus::VersionMismatch {
                        theirs,
                        ours: peer::PEER_PROTOCOL_VERSION,
                    };
                    return (StatusCode::BAD_GATEWAY, status.diagnosis(environment))
                        .into_response();
                }
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("unknown peer daemon {daemon_id}"),
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
