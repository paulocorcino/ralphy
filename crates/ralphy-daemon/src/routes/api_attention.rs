//! `GET /api/attention`: the attention list of this daemon and of its peers
//! (docs/adr/0085 D10). Each daemon serves its own list on `?local=1`; the
//! daemon the page talks to joins the peers' lists, as `/api/sessions` does.
//! The runs in the local list are the ones the attention tick read last
//! (`attention::tick`), which this module starts with the route.
//!
//! `POST /api/attention/dismiss` hides a request that may be hidden (D12),
//! on the daemon that owns it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use super::{RouterShared, UNANSWERED_HEADER};
use crate::attention::dismissals::Dismissals;
use crate::attention::done::DeskFiles;
use crate::attention::telegram::Notifier;
use crate::attention::tick::{self, Dismissed, Runs};
use crate::attention::{AttentionRequest, RunsRead};
use crate::routes::read_peer_store;
use crate::{fleet, peer, presence_board, session};

#[derive(serde::Deserialize)]
pub(crate) struct AttentionQuery {
    pub(crate) local: Option<u32>,
}

/// The body of `GET /api/attention`. `present` is only in the `?local=1`
/// reply: whether the operator is at one of the pages of the daemon that
/// answers (docs/adr/0085 D14). A main daemon reads it from each peer, and
/// an older peer, which has no such field, is "not present".
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct AttentionList {
    pub(crate) requests: Vec<AttentionRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) present: Option<bool>,
}

/// The reply of each peer in `peers_dir` to `GET <path>`, asked at the same
/// time.
pub(crate) async fn ask_peers(
    peers_dir: PathBuf,
    path: &str,
) -> Vec<(peer::PeerDescriptor, anyhow::Result<(u16, Vec<u8>)>)> {
    let (peers, _) = read_peer_store(peers_dir).await;
    futures_util::future::join_all(peers.into_iter().map(|peer| async move {
        let reply = peer::client::get(&peer, path).await;
        (peer, reply)
    }))
    .await
}

/// Name the peers in `unanswered` in the reply's `x-ralphy-unanswered`.
pub(crate) fn name_unanswered(response: &mut Response, unanswered: &[String]) {
    if unanswered.is_empty() {
        return;
    }
    match unanswered.join(",").parse() {
        Ok(value) => {
            response.headers_mut().insert(UNANSWERED_HEADER, value);
        }
        Err(e) => tracing::warn!(error = %e, "could not name the peers that did not answer"),
    }
}

/// One peer's own list, as its `?local=1` reply has it.
pub(crate) struct PeerList {
    pub(crate) daemon_id: String,
    pub(crate) list: AttentionList,
}

/// The own list of each peer in `peers_dir`, and the ids of the peers that
/// did not answer with one. An older peer has no such route, and is named
/// too.
pub(crate) async fn read_peer_lists(peers_dir: PathBuf) -> (Vec<PeerList>, Vec<String>) {
    let mut lists = Vec::new();
    let mut unanswered = Vec::new();
    for (peer, reply) in ask_peers(peers_dir, "/api/attention?local=1").await {
        let Ok((200, body)) = reply else {
            unanswered.push(peer.daemon_id);
            continue;
        };
        match serde_json::from_slice::<AttentionList>(&body) {
            Ok(list) => lists.push(PeerList {
                daemon_id: peer.daemon_id,
                list,
            }),
            Err(_) => {
                tracing::warn!(daemon_id = %peer.daemon_id, "peer returned an invalid attention list");
                unanswered.push(peer.daemon_id);
            }
        }
    }
    (lists, unanswered)
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

/// The runs of the last tick. A poisoned lock is logged and reads as none.
fn last_runs(runs: &Runs) -> RunsRead {
    match runs.lock() {
        Ok(runs) => runs.clone(),
        Err(e) => {
            tracing::warn!(error = %e, "the attention runs are poisoned");
            RunsRead::default()
        }
    }
}

/// This daemon's own list: its consoles now, the runs of the last tick, and
/// no dismissed request.
fn local_list(
    sessions: &session::SessionManager,
    runs: &Runs,
    dismissed: &Dismissed,
) -> Vec<AttentionRequest> {
    tick::list_with(sessions, &last_runs(runs), dismissed)
}

/// `GET /api/attention[?local=1]`.
pub(crate) async fn attention_route(
    sessions: Arc<session::SessionManager>,
    (runs, dismissed): (Runs, Dismissed),
    (peers_dir, presence): (PathBuf, presence_board::Shared),
    local_only: bool,
) -> Response {
    let mut requests = local_list(&sessions, &runs, &dismissed);
    if local_only {
        let present = presence_board::present_now(&presence);
        return Json(AttentionList {
            requests,
            present: Some(present),
        })
        .into_response();
    }
    // A peer that did not answer is named, never read as "nothing waits
    // there".
    let (lists, unanswered) = read_peer_lists(peers_dir).await;
    for PeerList { daemon_id, list } in lists {
        requests.extend(
            list.requests
                .into_iter()
                .map(|request| from_peer(&daemon_id, request)),
        );
    }
    let mut response = Json(AttentionList {
        requests,
        present: None,
    })
    .into_response();
    name_unanswered(&mut response, &unanswered);
    response
}

/// The body of `POST /api/attention/dismiss`.
#[derive(serde::Deserialize)]
pub(crate) struct DismissBody {
    pub(crate) id: String,
}

/// The text of a refusal to hide a live request.
const STILL_OPEN: &str = "the request is still open and cannot be hidden";

/// The owner of a request id: `Some((daemon_id, id on that daemon))` for a
/// peer's request. A local id (`console:`, `run:`, `issue:`) has no `/`.
fn peer_of(id: &str) -> Option<(&str, &str)> {
    id.split_once('/')
}

/// `POST /api/attention/dismiss {"id"}`: 200 for a request that may be
/// hidden, or one already hidden; 409 for a live request; 404 for an id
/// that names no request. A peer's request is dismissed on its peer.
pub(crate) async fn dismiss_route(
    sessions: Arc<session::SessionManager>,
    (runs, dismissed): (Runs, Dismissed),
    peers_dir: PathBuf,
    body: DismissBody,
) -> Response {
    match peer_of(&body.id) {
        Some((daemon_id, id)) => dismiss_on_peer(peers_dir, daemon_id, id).await,
        None => dismiss_local(&sessions, &runs, &dismissed, &body.id),
    }
}

fn dismiss_local(
    sessions: &session::SessionManager,
    runs: &Runs,
    dismissed: &Dismissed,
    id: &str,
) -> Response {
    let runs = last_runs(runs);
    let consoles = sessions.list_at(std::time::SystemTime::now());
    let mut dismissed = match dismissed.lock() {
        Ok(dismissed) => dismissed,
        Err(e) => {
            tracing::warn!(error = %e, "the attention dismissals are poisoned");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "the dismissals cannot be read",
            )
                .into_response();
        }
    };
    // An id already dismissed is dismissed again, which stores nothing: its
    // request may have ended since, and the answer is still 200.
    let request = crate::attention::list(&consoles, &runs, &|_| false)
        .into_iter()
        .find(|request| request.id == id);
    match request {
        Some(request) if !request.dismissable => (StatusCode::CONFLICT, STILL_OPEN).into_response(),
        None if !dismissed.contains(id) => {
            (StatusCode::NOT_FOUND, "unknown request").into_response()
        }
        _ => match dismissed.dismiss(id) {
            Ok(_) => Json(serde_json::json!({ "dismissed": true })).into_response(),
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "could not store a dismissal");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "the dismissal could not be stored",
                )
                    .into_response()
            }
        },
    }
}

/// Dismiss `id` on the peer `daemon_id`, and answer with its answer.
async fn dismiss_on_peer(peers_dir: PathBuf, daemon_id: &str, id: &str) -> Response {
    let (peers, rejects) = read_peer_store(peers_dir).await;
    let Some(peer) = peers.iter().find(|peer| peer.daemon_id == daemon_id) else {
        return (
            StatusCode::BAD_GATEWAY,
            fleet::unknown_daemon(daemon_id, &rejects),
        )
            .into_response();
    };
    let body = serde_json::json!({ "id": id });
    match peer::client::post_json(peer, "/api/attention/dismiss", &body).await {
        Ok((200, body)) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response(),
        Ok((404, _)) => (StatusCode::NOT_FOUND, "unknown request").into_response(),
        Ok((409, _)) => (StatusCode::CONFLICT, STILL_OPEN).into_response(),
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

/// `/api/attention` and `/api/attention/dismiss`, behind the same guard as
/// `/api/sessions`, and the attention tick that reads the runs for them. The
/// tick is started here, as the store watch is started where the router is
/// built: it needs the router's sessions and push bus, and a router built
/// with no runtime starts no tick. The dismissals are read once, from the
/// store of the repo registry.
pub(crate) fn attention_routes(s: &RouterShared) -> Router {
    let sessions = s.sessions.clone();
    let peers = s.peers_dir.clone();
    let presence = s.presence.clone();
    let runs = Runs::default();
    let dismissed: Dismissed = Arc::new(Mutex::new(Dismissals::in_store_of(&s.registry_path)));
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        let files = DeskFiles {
            desk: s.desk_path.clone(),
            registry: s.registry_path.clone(),
            peers: s.peer_repo_cache.clone(),
        };
        let telegram = Notifier::new(
            s.stores.telegram_config.clone(),
            (s.presence.clone(), s.telegram_claim.clone()),
            s.peers_dir.clone(),
            s.daemon_id.clone(),
        );
        runtime.spawn(tick::run(
            files,
            sessions.clone(),
            (runs.clone(), dismissed.clone()),
            (s.pushes.clone(), telegram),
            s.shutdown.clone(),
        ));
    }
    let read = {
        let (sessions, runs, dismissed, peers) = (
            sessions.clone(),
            runs.clone(),
            dismissed.clone(),
            peers.clone(),
        );
        move |Query(query): Query<AttentionQuery>| {
            attention_route(
                sessions.clone(),
                (runs.clone(), dismissed.clone()),
                (peers.clone(), presence.clone()),
                query.local == Some(1),
            )
        }
    };
    let dismiss = move |Json(body): Json<DismissBody>| {
        dismiss_route(
            sessions.clone(),
            (runs.clone(), dismissed.clone()),
            peers.clone(),
            body,
        )
    };
    Router::new()
        .route("/api/attention", get(read))
        .route("/api/attention/dismiss", post(dismiss))
}
