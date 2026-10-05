//! The read-only resources: repos, identity, about and agents.

use std::path::PathBuf;

use axum::extract::Query;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Json;
use axum::Router;

use super::read_peer_store;
use super::AgentLocator;
use super::RouterShared;
use crate::{dispatch, fleet, identity, peer, registry, rekey, roster};

#[derive(serde::Deserialize)]
pub(crate) struct AgentsQuery {
    pub(crate) repo: Option<String>,
}

/// `GET /api/repos`: the registered repos as JSON, each with its live
/// reachability. Read FRESH from disk on every request so a separate `ralphy
/// run` process's write shows up on the next page refresh. A load error yields
/// an empty list with `200` (logged) rather than failing the page. `branch` is
/// likewise read fresh from `<path>/.git/HEAD`, `None` when it cannot be
/// determined (detached HEAD, unreachable repo, worktree gitdir pointer).
///
/// The one place the registry self-heals (ADR-0036 amendment 2026-09-16): a
/// `path-<hash>` entry whose repo now HAS an origin is handed to `ralphy daemon
/// add` — synchronously, inside this request's blocking section — and the
/// registry is re-read, so the first page that could see the remote already
/// sees `owner/repo`. Once per `(slug, remote)` for this router's lifetime.
pub(crate) async fn repos_route(registry_path: PathBuf, memo: rekey::HealMemo) -> Response {
    #[derive(serde::Serialize)]
    struct RepoView {
        slug: String,
        // Additive: what the operator calls the project (`registry::project_name`).
        // The slug of a remoteless repo is a hash key, never a name.
        name: String,
        path: String,
        reachable: bool,
        // `Some` only on a branch; `head` tells a detached HEAD from no answer.
        branch: Option<String>,
        // Additive (#510): `{"kind":"branch","name"}` or `{"kind":"detached","sha"}`,
        // the shape of `sync.status`. Peers ignore unknown fields.
        head: Option<ralphy_git_read::Head>,
        // Additive (#204): the real working-tree state and origin URL. Both spawn
        // `git`, so the whole `Vec` is built inside `spawn_blocking` below.
        dirty: bool,
        remote: Option<String>,
        // Additive (#362): the canonical ABSOLUTE native root, for the explorer's
        // "Copy full path". `path` is left byte-identical — peers parse it
        // (`fleet::store_from_repos_json`).
        root: Option<String>,
    }
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry; serving empty list");
            registry::RegistryStore::default()
        }
    };
    fn build_views(store: &registry::RegistryStore) -> Vec<RepoView> {
        store
            .repos
            .iter()
            .map(|(slug, entry)| {
                let head = entry.head();
                RepoView {
                    slug: slug.clone(),
                    name: registry::project_name(slug, &entry.path),
                    path: entry.path.clone(),
                    reachable: entry.reachable(),
                    branch: match &head {
                        Some(ralphy_git_read::Head::Branch { name }) => Some(name.clone()),
                        _ => None,
                    },
                    head,
                    dirty: entry.dirty(),
                    remote: entry.remote(),
                    root: entry.root(),
                }
            })
            .collect()
    }
    // `head`/`dirty`/`remote` each spawn `git` per repo — that must not
    // block the async reactor, so the whole map runs on a blocking thread.
    let views = tokio::task::spawn_blocking(move || {
        let views = build_views(&store);
        let todo = rekey::claim_candidates(
            &memo,
            views
                .iter()
                .map(|v| (v.slug.as_str(), v.path.as_str(), v.remote.as_deref())),
        );
        if todo.is_empty() {
            return views;
        }
        rekey::heal(&dispatch::ProcessSpawner, &dispatch::ralphy_exe(), &todo);
        match registry::load_from(&registry_path) {
            Ok(healed) => build_views(&healed),
            Err(e) => {
                tracing::warn!(error = %e, "repo registry unreadable after a re-key; serving the pre-heal list");
                views
            }
        }
    })
    .await
    .unwrap_or_default();
    Json(views).into_response()
}

/// `GET /api/identity`: the loaded identity's `name`/`avatar` as JSON, or 404
/// when the daemon has not been baptized yet.
pub(crate) async fn identity_route(identity: Option<identity::Identity>) -> Response {
    #[derive(serde::Serialize)]
    struct IdentityView {
        name: String,
        avatar: String,
    }
    match identity {
        Some(id) => Json(IdentityView {
            name: id.name,
            avatar: id.avatar,
        })
        .into_response(),
        None => (StatusCode::NOT_FOUND, "no identity").into_response(),
    }
}

/// `GET /api/about`: the daemon's static product facts for the workbench About
/// panel — the git-published version (embedded at build time, so it tracks the
/// release tag) and the license/creator/source facts pulled straight from the
/// workspace manifest. Read-only, no secrets.
///
/// It carries no description. The one that was here was
/// `CARGO_PKG_DESCRIPTION`, written for whoever opens the manifest — it cites
/// an ADR path and the rule confining tokio to this crate — and an About card
/// is not that reader. The crate keeps its description; the card says nothing
/// rather than saying the wrong thing to the wrong audience.
pub(crate) async fn about_route() -> Response {
    #[derive(serde::Serialize)]
    struct AboutView {
        name: &'static str,
        version: &'static str,
        license: &'static str,
        repository: &'static str,
        creator: &'static str,
    }
    Json(AboutView {
        name: "ralphy",
        // Embedded by build.rs from `git describe --tags` (falls back to the
        // Cargo manifest version off a tarball).
        version: env!("RALPHY_VERSION"),
        // From the workspace manifest (`license`/`repository` are inherited).
        license: env!("CARGO_PKG_LICENSE"),
        repository: env!("CARGO_PKG_REPOSITORY"),
        creator: "Paulo Corcino",
    })
    .into_response()
}

/// `GET /api/agents[?repo=<routed-ref>]`: roster and presence snapshot from the
/// environment that owns `repo`. A peer request deliberately omits `repo`, so
/// the owning daemon computes locally and federation cannot recurse.
pub(crate) async fn agents_route(
    Query(query): Query<AgentsQuery>,
    peers_dir: PathBuf,
    daemon_id: String,
    locator: AgentLocator,
    bound_port: u16,
) -> Response {
    let Some(repo_ref) = query.repo.as_deref() else {
        return Json(roster::roster_with(locator.as_ref())).into_response();
    };
    let (descriptors, rejects) = read_peer_store(peers_dir).await;
    match fleet::route(repo_ref, &daemon_id, &descriptors) {
        fleet::Route::Local { .. } => Json(roster::roster_with(locator.as_ref())).into_response(),
        fleet::Route::Peer { peer, .. } => {
            let status = peer::client::probe(
                peer,
                peer::client::SelfRef {
                    port: bound_port,
                    daemon_id: &daemon_id,
                },
            )
            .await;
            if status != peer::client::PeerStatus::Reachable {
                return (StatusCode::BAD_GATEWAY, status.diagnosis(&peer.environment))
                    .into_response();
            }
            match peer::client::get(peer, "/api/agents").await {
                Ok((200, body)) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/json")],
                    body,
                )
                    .into_response(),
                Ok((status, _)) => (
                    StatusCode::BAD_GATEWAY,
                    fleet::peer_unreachable(
                        peer,
                        &format!("the peer answered HTTP {status} to the roster request"),
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
        fleet::Route::UnknownDaemon { daemon_id } => (
            StatusCode::BAD_GATEWAY,
            fleet::unknown_daemon(daemon_id, &rejects),
        )
            .into_response(),
    }
}

/// The read routes of the workbench: identity, about, agents and repos.
pub(crate) fn read_routes(s: &RouterShared) -> Router {
    let identity = s.identity.clone();
    let agents_peers = s.peers_dir.clone();
    let agents_daemon_id = s.daemon_id.clone().unwrap_or_default();
    let roster_locator = s.roster_locator.clone();
    let bound_port = s.bound_port;
    Router::new()
        .route("/api/identity", get(move || identity_route(identity)))
        .route("/api/about", get(about_route))
        .route(
            "/api/agents",
            get(move |query: Query<AgentsQuery>| {
                agents_route(
                    query,
                    agents_peers.clone(),
                    agents_daemon_id.clone(),
                    roster_locator.clone(),
                    bound_port,
                )
            }),
        )
        .route(
            "/api/repos",
            get({
                let p = s.registry_path.clone();
                let memo = s.heal_memo.clone();
                move || repos_route(p, memo)
            }),
        )
}
