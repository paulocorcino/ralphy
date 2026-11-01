//! `/api/sessions` and `/api/sessions/close`: the session list across the
//! fleet and the close verb, relayed to the owning peer when needed.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;

use super::RouterShared;
use crate::peer::join::{ask_peers, name_unanswered};
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
    let replies = ask_peers(peers_dir, "/api/sessions?local=1").await;
    // The rows keep their own parse and their own repo rule: the peer's id in
    // front of the repo ref is the owner key of the session. The join only
    // asks the peers and names the ones that did not answer.
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
    name_unanswered(&mut response, &unanswered);
    response
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
            return close_local(&sessions, q.id);
        }
        let daemon_id = identity
            .as_ref()
            .map(|identity| identity.id.to_string())
            .unwrap_or_default();
        let (peers, rejects) = read_peer_store(peers_dir).await;
        match fleet::route(repo_ref, &daemon_id, &peers) {
            fleet::route::Route::Peer { peer, .. } => return close_on_peer(peer, q.id).await,
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
    close_local(&sessions, q.id)
}

fn close_local(sessions: &session::SessionManager, id: u64) -> Response {
    if sessions.close(id) {
        Json(serde_json::json!({ "closed": true })).into_response()
    } else {
        (StatusCode::NOT_FOUND, "unknown session").into_response()
    }
}

/// Ask the peer that owns a session to close it, and answer with its reply.
async fn close_on_peer(peer: &peer::PeerDescriptor, id: u64) -> Response {
    let path = format!("/api/sessions/close?id={id}");
    match peer::client::post_json(peer, &path, &serde_json::json!({})).await {
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
            peer::client::transport_failed(peer, format!("{error:#}")).await,
        )
            .into_response(),
    }
}

/// `/api/sessions` and `/api/sessions/close`. A console opens on a channel
/// of the page socket.
pub(crate) fn session_routes(s: &RouterShared) -> Router {
    Router::new()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A relaying daemon parses a peer's row and sends it on: the silent
    /// block keeps its source and reason, and gains no state.
    #[test]
    fn a_silent_agent_block_survives_the_relay() {
        let row = HostedSessionInfo {
            id: 3,
            repo: "owner/r".to_string(),
            agent: "claude".to_string(),
            kind: "agent".to_string(),
            started_at: 1,
            daemon_id: "d".to_string(),
            environment: "wsl".to_string(),
            name: None,
            checkout: None,
            record: None,
            agent_state: Some(agent_state::AgentState::silent(
                "the hook did not start".to_string(),
                std::time::SystemTime::UNIX_EPOCH,
            )),
        };
        let wire = serde_json::to_string(&row).expect("serialize");
        let back: HostedSessionInfo = serde_json::from_str(&wire).expect("parse");
        let again: serde_json::Value =
            serde_json::to_value(&back).expect("serialize the parsed row");
        let block = &again["agent_state"];
        assert_eq!(block["source"], "silent", "{again}");
        assert_eq!(block["reason"], "the hook did not start", "{again}");
        assert_eq!(block["since"], "1970-01-01T00:00:00Z", "{again}");
        assert!(block.get("state").is_none(), "{again}");
    }

    /// A peer that answers with no session list is named in
    /// `x-ralphy-unanswered`, never listed as a peer with no sessions.
    #[tokio::test]
    async fn sessions_name_a_peer_that_does_not_answer() {
        /// The reply of the fake peer: a 200 whose body is no session list.
        async fn answer(mut socket: tokio::net::TcpStream) -> std::io::Result<()> {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut request = [0u8; 1024];
            if socket.read(&mut request).await? == 0 {
                return Ok(());
            }
            let reply = concat!(
                "HTTP/1.1 200 OK",
                "\r\nContent-Length: 2",
                "\r\nConnection: close",
                "\r\n\r\n{}"
            );
            socket.write_all(reply.as_bytes()).await
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a fake peer");
        let port = listener.local_addr().expect("the fake peer address").port();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                if let Err(e) = answer(socket).await {
                    tracing::warn!(error = %e, "the fake peer could not answer");
                }
            }
        });
        let peers_dir = tempfile::tempdir().expect("a peers folder");
        let peer = peer::PeerDescriptor {
            daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX".to_string(),
            name: "peer".to_string(),
            avatar: String::new(),
            address: "127.0.0.1".to_string(),
            port,
            environment: "wsl".to_string(),
            os: String::new(),
            token: "peer-token".to_string(),
            protocol_version: peer::PEER_PROTOCOL_VERSION,
            nudge: None,
            tunnel: None,
        };
        peer::write_descriptor(peers_dir.path(), &peer).expect("write the peer descriptor");
        let response = sessions_route(
            Arc::new(session::SessionManager::new()),
            peers_dir.path().join("peers"),
            None,
            "windows".to_string(),
            false,
        )
        .await;
        let named = response
            .headers()
            .get(peer::join::UNANSWERED_HEADER)
            .and_then(|value| value.to_str().ok());
        assert_eq!(named, Some(peer.daemon_id.as_str()));
    }
}
