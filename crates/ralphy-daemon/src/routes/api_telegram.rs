//! `GET /api/telegram`: the Telegram state of this daemon (docs/adr/0085
//! D14), and of each of its peers, which the daemon the page talks to joins
//! as it joins `/api/attention`. `?local=1` is this daemon's own state: what
//! a main daemon reads from each peer. The config is read at each request.
//!
//! `POST /api/telegram/main {"daemon_id"}` is the claim of a main daemon on
//! this one (`peer::telegram_claim`, which this module starts): while it
//! holds, this daemon sends no message, and the main sends its console
//! requests.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use axum::extract::{DefaultBodyLimit, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use super::{ask_peers, name_unanswered, RouterShared};
use crate::peer::telegram_claim;
use crate::telegram_state::{self, SharedClaim, TelegramState};

/// The longest daemon id a claim may name. A daemon id is a ULID (26
/// characters); the rest is room.
const MAX_MAIN_ID_BYTES: usize = 64;

/// The largest claim body the route reads (ADR-0072 D11). The body is one
/// object with an id of at most [`MAX_MAIN_ID_BYTES`] bytes.
const CLAIM_MAX_BYTES: usize = 256;

#[derive(serde::Deserialize)]
pub(crate) struct TelegramQuery {
    pub(crate) local: Option<u32>,
}

/// The body of `POST /api/telegram/main`.
#[derive(serde::Deserialize)]
pub(crate) struct ClaimBody {
    pub(crate) daemon_id: String,
}

/// What the routes read: the config, the claim on this daemon, the peers,
/// and this daemon's id and name.
#[derive(Clone)]
pub(crate) struct TelegramShared {
    pub(crate) config: PathBuf,
    pub(crate) claim: SharedClaim,
    pub(crate) peers_dir: PathBuf,
    pub(crate) daemon_id: Option<String>,
    pub(crate) name: Option<String>,
}

/// The reply of `GET /api/telegram`. `main_name` is the name of the main
/// daemon when the daemon that answers knows it; `peers` is only in the
/// joined reply.
#[derive(serde::Serialize)]
struct TelegramView {
    #[serde(flatten)]
    state: TelegramState,
    #[serde(skip_serializing_if = "Option::is_none")]
    main_name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    peers: Vec<PeerView>,
}

/// One peer's state in the joined reply.
#[derive(serde::Serialize)]
struct PeerView {
    daemon_id: String,
    environment: String,
    #[serde(flatten)]
    state: TelegramState,
    #[serde(skip_serializing_if = "Option::is_none")]
    main_name: Option<String>,
}

/// The state of this daemon now, read off the runtime: the config is a file.
async fn own_state(t: &TelegramShared) -> Option<TelegramState> {
    let (config, claim) = (t.config.clone(), t.claim.clone());
    super::blocking_read(move || telegram_state::state_with(&config, &claim, SystemTime::now()))
        .await
}

/// The name of the main daemon of `state`, from `names` (daemon id → name).
fn main_name(state: &TelegramState, names: &HashMap<String, String>) -> Option<String> {
    match state {
        TelegramState::ThroughMain { main } => names.get(main).cloned(),
        TelegramState::Off | TelegramState::Sends => None,
    }
}

/// Each peer's own state, in the joined reply; the names of the daemons
/// this daemon knows, by id; and the peers that did not answer. A peer that
/// did not answer, or an older one with no such route, is named, never shown
/// as "off".
async fn peer_states(t: &TelegramShared) -> (HashMap<String, String>, Vec<PeerView>, Vec<String>) {
    let mut names: HashMap<String, String> = HashMap::new();
    if let (Some(id), Some(name)) = (&t.daemon_id, &t.name) {
        names.insert(id.clone(), name.clone());
    }
    let replies = ask_peers(t.peers_dir.clone(), "/api/telegram?local=1").await;
    names.extend(
        replies
            .iter()
            .map(|(peer, _)| (peer.daemon_id.clone(), peer.name.clone())),
    );
    let mut unanswered = Vec::new();
    let mut peers = Vec::new();
    for (peer, reply) in replies {
        match parse_state(reply) {
            Some(state) => peers.push(PeerView {
                main_name: main_name(&state, &names),
                daemon_id: peer.daemon_id,
                environment: peer.environment,
                state,
            }),
            None => unanswered.push(peer.daemon_id),
        }
    }
    (names, peers, unanswered)
}

/// The state a peer answered, when it answered one.
fn parse_state(reply: anyhow::Result<(u16, Vec<u8>)>) -> Option<TelegramState> {
    match reply {
        Ok((200, body)) => serde_json::from_slice(&body).ok(),
        _ => None,
    }
}

/// `GET /api/telegram[?local=1]`.
pub(crate) async fn telegram_route(t: TelegramShared, local_only: bool) -> Response {
    let Some(state) = own_state(&t).await else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the Telegram config could not be read",
        )
            .into_response();
    };
    if local_only {
        return Json(state).into_response();
    }
    let (names, peers, unanswered) = peer_states(&t).await;
    let mut response = Json(TelegramView {
        main_name: main_name(&state, &names),
        state,
        peers,
    })
    .into_response();
    name_unanswered(&mut response, &unanswered);
    response
}

/// Whether `id` may be the id of a main daemon: short, and only the
/// characters of a daemon id. It is shown on the page.
fn main_id_is_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_MAIN_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// Whether this daemon refuses the claim of `main` because it is the main of
/// `main` itself: it has its own bot, `main` is one of its peers, and its id
/// comes first. Two daemons that are each other's peers would otherwise
/// claim each other, and neither would send.
fn mutual_and_first(t: &TelegramShared, main: &str) -> bool {
    let Some(own) = &t.daemon_id else {
        return false;
    };
    let has_bot = matches!(telegram_state::bot_at(&t.config), Ok(Some(_)));
    let (peers, _) = crate::peer::read_store(&t.peers_dir);
    own.as_str() < main && has_bot && peers.iter().any(|p| p.daemon_id == main)
}

/// `POST /api/telegram/main {"daemon_id"}`: 200 with the state after the
/// claim; 400 for an id that is not one; 409 when this daemon is the main of
/// the claimer.
pub(crate) async fn claim_route(t: TelegramShared, body: ClaimBody) -> Response {
    let main = body.daemon_id;
    if !main_id_is_valid(&main) || t.daemon_id.as_deref() == Some(main.as_str()) {
        return (StatusCode::BAD_REQUEST, "not the id of another daemon").into_response();
    }
    let refuse = {
        let (t, main) = (t.clone(), main.clone());
        super::blocking_read(move || mutual_and_first(&t, &main)).await
    };
    match refuse {
        Some(false) => {}
        Some(true) => {
            return (StatusCode::CONFLICT, "this daemon sends for that daemon").into_response()
        }
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "the claim could not be checked",
            )
                .into_response()
        }
    }
    match t.claim.lock() {
        Ok(mut claim) => claim.take(main, SystemTime::now()),
        Err(e) => {
            tracing::warn!(error = %e, "the Telegram claim is poisoned");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "the claim could not be kept",
            )
                .into_response();
        }
    }
    telegram_route(t, true).await
}

/// `/api/telegram` and `/api/telegram/main`, behind the same guard as every
/// route, and the claim a main daemon sends to its peers. The claim is
/// started here, as the attention tick is started with its route: a router
/// built with no runtime, or with no identity, claims nothing.
pub(crate) fn telegram_routes(s: &RouterShared) -> Router {
    let t = TelegramShared {
        config: s.stores.telegram_config.clone(),
        claim: s.telegram_claim.clone(),
        peers_dir: s.peers_dir.clone(),
        daemon_id: s.daemon_id.clone(),
        name: s.identity.as_ref().map(|i| i.name.clone()),
    };
    if let (Ok(runtime), Some(daemon_id)) =
        (tokio::runtime::Handle::try_current(), t.daemon_id.clone())
    {
        runtime.spawn(telegram_claim::run(
            telegram_claim::Claimer {
                config: t.config.clone(),
                claim: t.claim.clone(),
                peers_dir: t.peers_dir.clone(),
                daemon_id,
            },
            s.shutdown.clone(),
        ));
    }
    let read = {
        let t = t.clone();
        move |Query(query): Query<TelegramQuery>| telegram_route(t.clone(), query.local == Some(1))
    };
    let claim = move |Json(body): Json<ClaimBody>| claim_route(t.clone(), body);
    Router::new().route("/api/telegram", get(read)).route(
        "/api/telegram/main",
        post(claim).layer(DefaultBodyLimit::max(CLAIM_MAX_BYTES)),
    )
}

#[cfg(test)]
mod tests;
