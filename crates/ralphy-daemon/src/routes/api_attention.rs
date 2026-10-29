//! `GET /api/attention`: the attention list of this daemon and of its peers
//! (docs/adr/0085 D10). Each daemon serves its own list on `?local=1`; the
//! daemon the page talks to joins the peers' lists, as `/api/sessions` does.
//! The runs in the local list are the ones the attention tick read last
//! (`attention::tick`), which this module starts with the route.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use axum::extract::Query;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use super::{RouterShared, UNANSWERED_HEADER};
use crate::attention::tick::{self, Runs};
use crate::attention::{self, AttentionRequest};
use crate::routes::read_peer_store;
use crate::{peer, session};

#[derive(serde::Deserialize)]
pub(crate) struct AttentionQuery {
    pub(crate) local: Option<u32>,
}

/// The body of `GET /api/attention`.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct AttentionList {
    pub(crate) requests: Vec<AttentionRequest>,
}

/// A peer's request as the page reads it: `id`, `console` and `repo` carry
/// the peer's daemon id, so two daemons' `console:1` stay two requests and
/// the page finds the console under the peer's repo ref.
fn from_peer(daemon_id: &str, mut request: AttentionRequest) -> AttentionRequest {
    let prefix = |value: &str| format!("{daemon_id}/{value}");
    request.id = format!("{daemon_id}/{}", request.id);
    request.console = request.console.as_deref().map(prefix);
    request.repo = request.repo.as_deref().map(prefix);
    request
}

/// This daemon's own list: its consoles now, and the runs of the last tick.
fn local_list(sessions: &session::SessionManager, runs: &Runs) -> Vec<AttentionRequest> {
    let consoles = sessions.list_at(SystemTime::now());
    match runs.lock() {
        Ok(runs) => attention::list(&consoles, &runs),
        Err(e) => {
            tracing::warn!(error = %e, "the attention runs are poisoned");
            attention::list(&consoles, &[])
        }
    }
}

/// `GET /api/attention[?local=1]`.
pub(crate) async fn attention_route(
    sessions: Arc<session::SessionManager>,
    runs: Runs,
    peers_dir: PathBuf,
    local_only: bool,
) -> Response {
    let mut requests = local_list(&sessions, &runs);
    if local_only {
        return Json(AttentionList { requests }).into_response();
    }
    let (peers, _) = read_peer_store(peers_dir).await;
    let replies = futures_util::future::join_all(peers.into_iter().map(|peer| async move {
        let reply = peer::client::get(&peer, "/api/attention?local=1").await;
        (peer, reply)
    }))
    .await;
    // A peer that did not answer is named, never read as "nothing waits
    // there". An older peer has no such route, and is named too.
    let mut unanswered = Vec::new();
    for (peer, reply) in replies {
        let Ok((200, body)) = reply else {
            unanswered.push(peer.daemon_id);
            continue;
        };
        let Ok(list) = serde_json::from_slice::<AttentionList>(&body) else {
            tracing::warn!(daemon_id = %peer.daemon_id, "peer returned an invalid attention list");
            unanswered.push(peer.daemon_id);
            continue;
        };
        requests.extend(
            list.requests
                .into_iter()
                .map(|request| from_peer(&peer.daemon_id, request)),
        );
    }
    let mut response = Json(AttentionList { requests }).into_response();
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

/// `/api/attention`, behind the same guard as `/api/sessions`, and the
/// attention tick that reads the runs for it. The tick is started here, as
/// the store watch is started where the router is built: it needs the
/// router's sessions and push bus, and a router built with no runtime starts
/// no tick.
pub(crate) fn attention_routes(s: &RouterShared) -> Router {
    let sessions = s.sessions.clone();
    let peers = s.peers_dir.clone();
    let runs = Runs::default();
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(tick::run(
            s.registry_path.clone(),
            sessions.clone(),
            runs.clone(),
            s.pushes.clone(),
            s.shutdown.clone(),
        ));
    }
    Router::new().route(
        "/api/attention",
        get(move |Query(query): Query<AttentionQuery>| {
            attention_route(
                sessions.clone(),
                runs.clone(),
                peers.clone(),
                query.local == Some(1),
            )
        }),
    )
}
