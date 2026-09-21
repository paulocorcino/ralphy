//! `/ws/session` and `/api/sessions*`: launch, reattach, watch and close the
//! daemon-owned sessions, locally or relayed to the peer that owns them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

mod bridge;
mod relay;

pub(crate) use bridge::*;
pub(crate) use relay::*;

use super::{blocking_read, read_peer_store};
use crate::{agent_state, auth, checkout, confine, fleet, identity, peer, registry, session};

/// Query for `/ws/session`. A NEW agent launch carries `repo` + `agent`; a NEW
/// free-console launch (issue #167) carries `console=1` and an optional `repo`
/// (home dir when absent); a REATTACH carries `id` (and optional `takeover=1`,
/// or `watch=1` for a read-only attach, issue #334). All optional so one struct
/// serves every shape; the handler dispatches on `id` first, then `console`.
#[derive(serde::Deserialize)]
pub(crate) struct SessionQuery {
    pub(crate) repo: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) id: Option<u64>,
    pub(crate) takeover: Option<u32>,
    pub(crate) watch: Option<u32>,
    pub(crate) console: Option<u32>,
    /// A worktree NAME beside `repo`+`agent` on a NEW agent launch (ADR-0063
    /// §3); ignored on a reattach — the record owns it — and on `console=1`.
    pub(crate) checkout: Option<String>,
}

/// The two labels a `session-open` frame carries beside the identity: the
/// vendor-side name (Claude only) and the worktree the console lives in. They
/// travel together on every path — launch, reattach, watch.
#[derive(Default)]
pub(crate) struct SessionLabels {
    pub(crate) name: Option<String>,
    pub(crate) checkout: Option<String>,
}

#[derive(Clone)]
pub(crate) struct SessionHost {
    pub(crate) peers_dir: PathBuf,
    pub(crate) identity: Option<identity::Identity>,
    pub(crate) environment: String,
    /// The port this daemon bound, so a peer probe can refuse to dial itself.
    pub(crate) bound_port: u16,
}

/// `GET /ws/session`: four shapes over one route.
///
/// - `?id=<id>[&takeover=1]` — REATTACH to a daemon-owned session. `attach`
///   returns `404` for an unknown id and `409` for a busy one (a single writer is
///   attached and `takeover` was not set) — both BEFORE the upgrade, so a refusal
///   is an HTTP status the browser can read, not a silently-dropped socket.
/// - `?id=<id>&watch=1` — REATTACH read-only (issue #334): the same replay and
///   live stream, but the writer slot is never claimed, so a busy session is
///   reachable (never `409`) and nobody is evicted. Only `404` refuses it. This
///   is what lets a second workbench see a session instead of stealing it.
/// - `?repo=<slug>&agent=<claude|codex|opencode>[&checkout=<name>]` — NEW agent
///   launch. Rejects (`400`) an unknown agent, an unreadable registry, or an
///   unregistered slug before upgrading; an unknown or malformed `checkout` is
///   `400 unknown checkout` before anything is written or spawned (ADR-0063
///   §3); a spawn failure is `500`.
/// - `?console=1[&repo=<slug>]` — NEW free-console launch (issue #167): the
///   platform shell in the chosen repo's dir, or the home dir when `repo` is
///   absent. Rejects (`400`) an unreadable registry or an unregistered slug;
///   a spawn failure is `500`.
pub(crate) async fn session_ws_upgrade(
    ws: WebSocketUpgrade,
    Query(mut query): Query<SessionQuery>,
    sessions: Arc<session::SessionManager>,
    registry_path: PathBuf,
    host: SessionHost,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Response {
    let SessionHost {
        peers_dir,
        identity,
        environment,
        bound_port,
    } = host;
    let daemon_id = identity
        .as_ref()
        .map(|identity| identity.id.to_string())
        .unwrap_or_default();
    // A peer free console is the one composite-ref session hosted HERE. Match
    // both id and repo so an equal numeric id owned by the peer still proxies.
    let locally_owned = query.id.and_then(|id| {
        sessions.get(id).filter(|info| {
            query
                .repo
                .as_deref()
                .map(|repo| repo == info.repo)
                .unwrap_or(true)
        })
    });
    if query.console != Some(1) && locally_owned.is_none() {
        if let Some(repo_ref) = query.repo.clone() {
            let (descriptors, rejects) = read_peer_store(peers_dir.clone()).await;
            match fleet::route(&repo_ref, &daemon_id, &descriptors) {
                fleet::route::Route::Local { slug } => {
                    query.repo = Some(slug.to_string());
                }
                fleet::route::Route::Peer { peer, slug } => {
                    let peer_query = peer_session_query(&query, slug);
                    let me = peer::client::SelfRef {
                        port: bound_port,
                        daemon_id: &daemon_id,
                    };
                    return match peer::client::session(peer, &peer_query, me).await {
                        Ok(peer_socket) => ws.on_upgrade(move |socket| {
                            peer_session_ws(socket, peer_socket, shutdown)
                        }),
                        Err(peer::client::SocketError::Peer(status)) => {
                            (StatusCode::BAD_GATEWAY, status.diagnosis(&peer.environment))
                                .into_response()
                        }
                        Err(peer::client::SocketError::Http { status, body }) => (
                            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
                            body,
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
            }
        }
    }
    if let Some(id) = query.id {
        let effective_environment = locally_owned
            .as_ref()
            .and_then(|info| info.environment.clone())
            .unwrap_or_else(|| environment.clone());
        // A reattach re-announces the name the child was LAUNCHED under; the spec
        // is long gone, so the session record is where it comes from. Without
        // this a reload would blank the name on a console that still answers to it.
        let effective_labels = locally_owned
            .as_ref()
            .map(|info| SessionLabels {
                name: info.name.clone(),
                checkout: info.checkout.clone(),
            })
            .unwrap_or_default();
        // A watcher never touches the writer slot, so it is dispatched BEFORE the
        // attach branch and can never produce a `409`.
        if query.watch == Some(1) {
            return match sessions.watch(id) {
                Ok(att) => ws.on_upgrade(move |socket| {
                    session_ws(
                        socket,
                        att,
                        id,
                        daemon_id,
                        effective_environment,
                        effective_labels,
                        shutdown,
                    )
                }),
                // `watch` never yields `Busy`; matching the variant keeps that a
                // compile-time fact rather than a comment.
                Err(session::AttachError::Unknown) => {
                    (StatusCode::NOT_FOUND, "unknown session").into_response()
                }
                Err(session::AttachError::Busy) => {
                    (StatusCode::CONFLICT, "session busy").into_response()
                }
            };
        }
        return match sessions.attach(id, query.takeover == Some(1)) {
            Ok(att) => ws.on_upgrade(move |socket| {
                session_ws(
                    socket,
                    att,
                    id,
                    daemon_id,
                    effective_environment,
                    effective_labels,
                    shutdown,
                )
            }),
            Err(session::AttachError::Unknown) => {
                (StatusCode::NOT_FOUND, "unknown session").into_response()
            }
            Err(session::AttachError::Busy) => {
                (StatusCode::CONFLICT, "session busy").into_response()
            }
        };
    }
    if query.console == Some(1) {
        if let Some(repo_ref) = query.repo.clone() {
            let (descriptors, rejects) = read_peer_store(peers_dir).await;
            match fleet::route(&repo_ref, &daemon_id, &descriptors) {
                fleet::route::Route::Local { slug } => {
                    query.repo = Some(slug.to_string());
                }
                fleet::route::Route::Peer { peer, slug } => {
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
                    let Some(nudge) = peer.nudge.as_ref() else {
                        return (
                            StatusCode::BAD_GATEWAY,
                            format!(
                                "{} cannot host a free console: peer advertises no WSL distro",
                                peer.environment
                            ),
                        )
                            .into_response();
                    };
                    let Some(launcher) = session::peer_console_launcher() else {
                        return (
                            StatusCode::BAD_GATEWAY,
                            format!(
                                "{} cannot host a free console: wsl.exe launcher not found",
                                peer.environment
                            ),
                        )
                            .into_response();
                    };
                    let entry = match peer::client::get(peer, "/api/repos").await {
                        Ok((200, body)) => match fleet::repo_from_repos_json(&body, slug) {
                            Ok(Some(entry)) => entry,
                            Ok(None) => {
                                return (
                                    StatusCode::BAD_REQUEST,
                                    format!("{} has no repository {slug}", peer.environment),
                                )
                                    .into_response();
                            }
                            Err(_) => {
                                return (
                                    StatusCode::BAD_GATEWAY,
                                    format!(
                                        "{} returned an unreadable repository list",
                                        peer.environment
                                    ),
                                )
                                    .into_response();
                            }
                        },
                        Ok((status, _)) => {
                            return (
                                StatusCode::BAD_GATEWAY,
                                format!(
                                    "{} refused its repository list with HTTP {status}",
                                    peer.environment
                                ),
                            )
                                .into_response();
                        }
                        Err(error) => {
                            return (
                                StatusCode::BAD_GATEWAY,
                                fleet::route::peer_unreachable(peer, &format!("{error:#}")),
                            )
                                .into_response();
                        }
                    };
                    if entry.path.is_empty() {
                        return (
                            StatusCode::BAD_REQUEST,
                            format!("{} returned an empty path for {slug}", peer.environment),
                        )
                            .into_response();
                    }
                    if !entry.reachable {
                        return (
                            StatusCode::BAD_REQUEST,
                            format!(
                                "{} reports repository {slug} path unreachable",
                                peer.environment
                            ),
                        )
                            .into_response();
                    }
                    let spec = session::peer_console_spec(
                        launcher,
                        &nudge.distro,
                        Path::new(&entry.path),
                        24,
                        80,
                    );
                    let effective_environment = peer.environment.clone();
                    return match sessions.spawn_attached(
                        repo_ref.clone(),
                        "console".to_string(),
                        "console".to_string(),
                        Some(effective_environment.clone()),
                        None,
                        spec,
                    ) {
                        Ok((id, att)) => ws.on_upgrade(move |socket| {
                            session_ws(
                                socket,
                                att,
                                id,
                                daemon_id,
                                effective_environment,
                                SessionLabels::default(),
                                shutdown,
                            )
                        }),
                        Err(error) => {
                            tracing::warn!(
                                environment = %peer.environment,
                                error = %error,
                                "failed to spawn a peer free console"
                            );
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!(
                                    "{} free-console launcher failed: {error}",
                                    peer.environment
                                ),
                            )
                                .into_response()
                        }
                    };
                }
                fleet::route::Route::UnknownDaemon { daemon_id } => {
                    if let Some((peer_environment, theirs)) = rejects
                        .iter()
                        .find_map(|reject| reject.version_mismatch_for(daemon_id))
                    {
                        let status = peer::client::PeerStatus::VersionMismatch {
                            theirs,
                            ours: peer::PEER_PROTOCOL_VERSION,
                        };
                        return (StatusCode::BAD_GATEWAY, status.diagnosis(peer_environment))
                            .into_response();
                    }
                    return (
                        StatusCode::BAD_GATEWAY,
                        format!("unknown peer daemon {daemon_id}"),
                    )
                        .into_response();
                }
            }
        }
        let repo_path = match query.repo.as_deref() {
            Some(slug) => {
                let store = match registry::load_from(&registry_path) {
                    Ok(store) => store,
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to load repo registry for a console session");
                        return (StatusCode::BAD_REQUEST, "repo registry unreadable")
                            .into_response();
                    }
                };
                let Some(entry) = store.entry(slug) else {
                    return (StatusCode::BAD_REQUEST, "unknown repo").into_response();
                };
                Some(PathBuf::from(&entry.path))
            }
            None => None,
        };
        let cwd = session::console_cwd(repo_path);
        let spec = session::console_spec(cwd, 24, 80);
        let repo_label = query.repo.clone().unwrap_or_else(|| "~".to_string());
        return match sessions.spawn_attached(
            repo_label,
            "console".to_string(),
            "console".to_string(),
            None,
            None,
            spec,
        ) {
            Ok((id, att)) => ws.on_upgrade(move |socket| {
                session_ws(
                    socket,
                    att,
                    id,
                    daemon_id,
                    environment,
                    SessionLabels::default(),
                    shutdown,
                )
            }),
            Err(e) => {
                tracing::warn!(error = %e, "failed to spawn a console session");
                (StatusCode::INTERNAL_SERVER_ERROR, "failed to spawn session").into_response()
            }
        };
    }
    let Some(agent_str) = query.agent.as_deref() else {
        return (StatusCode::BAD_REQUEST, "unknown agent").into_response();
    };
    let Some(agent) = session::Agent::from_query(agent_str) else {
        return (StatusCode::BAD_REQUEST, "unknown agent").into_response();
    };
    let Some(repo) = query.repo.as_deref() else {
        return (StatusCode::BAD_REQUEST, "unknown repo").into_response();
    };
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry for a session");
            return (StatusCode::BAD_REQUEST, "repo registry unreadable").into_response();
        }
    };
    let Some(entry) = store.entry(repo) else {
        return (StatusCode::BAD_REQUEST, "unknown repo").into_response();
    };
    let root = PathBuf::from(&entry.path);
    // ADR-0063 §3: the selected checkout, resolved with the same resolver and
    // confinement as every git-backed verb (`spawn_cwd`), off the runtime.
    // INVARIANT: this returns BEFORE the Cursor gate, the Gemini gate, `spec_for`
    // and every spawn — a bad name writes nothing and spawns nothing.
    let checkout = match query.checkout.as_deref() {
        None => None,
        Some(name) => {
            let (name, primary) = (name.to_string(), root.clone());
            let resolved = blocking_read(move || {
                let c = checkout::resolve(&name, |n| checkout::is_linked(&primary, n))
                    .map_err(|_| ())?;
                confine::confine(&primary, &c.prefix("")).map_err(|_| ())?;
                Ok::<_, ()>(c)
            })
            .await;
            match resolved {
                Some(Ok(c)) => Some(c),
                Some(Err(())) => {
                    return (StatusCode::BAD_REQUEST, checkout::UNKNOWN).into_response()
                }
                None => return (StatusCode::INTERNAL_SERVER_ERROR, "unavailable").into_response(),
            }
        }
    };
    let cwd = checkout
        .as_ref()
        .map_or_else(|| root.clone(), |c| c.dir(&root));
    // ADR-0042 D6: an ordinary Cursor run uploads the enclosing repository. The
    // run path is gated in the adapter, but this interactive launch spawns
    // `cursor-agent` directly — so the gate has to run here too, BEFORE the spec
    // is built and anything is spawned: it writes `.cursorindexingignore` into the
    // unprotected repo (announced on the daemon log) and then proceeds. A write
    // failure (read-only tree) is the only way it stops the launch. The gate
    // walks up from `cwd`, so a checkout gets its own opt-out and the primary
    // keeps its own (ADR-0063 §3); the opt-in is the primary's `.ralphy/`.
    if agent == session::Agent::Cursor {
        if let Err(e) =
            ralphy_proc_util::cursor::indexing_gate(&cwd, session::cursor_indexing_allowed(&root))
        {
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    }
    // ADR-0043 D4/D6: a Gemini child is contained by an owned configuration root
    // AND the policy document inside it. The daemon may not import the adapter
    // (ADR-0032 §10), so it cannot GENERATE that document — and duplicating the
    // generator would drift from the operator's imported deny rules. It therefore
    // fails closed. INVARIANT: this refusal precedes `spec_for` and every spawn
    // path, so no Gemini child is ever created outside the owned root.
    if agent == session::Agent::Gemini && !session::gemini_policy_path(&root).is_file() {
        // The remedy names ONLY the run verb: `ralphy init`'s login probe calls
        // `root::ensure` directly and writes no policy document
        // (`ralphy-agent-gemini/src/lib.rs` — `write_policy` is reached only from
        // `prepare_root`), so naming it here would send the operator round a loop
        // that ends in this same refusal.
        return (
            StatusCode::BAD_REQUEST,
            "gemini: no owned configuration root in this repo — run `ralphy run --agent gemini` here first (`ralphy init` alone does not write the policy document)",
        )
            .into_response();
    }
    // The id first: the agent-state files are named by it and must exist
    // before the child that reads them is launched (ADR-0059 §5). Only a
    // vendor with hooks gets the slot; the store dir failing to resolve means
    // no hooks, never no console.
    let id = sessions.reserve_id();
    let status = match agent {
        session::Agent::Claude => auth::store_dir()
            .ok()
            .map(|d| agent_state::StatusFiles::for_session(&d.join("sessions"), id)),
        _ => None,
    };
    let spec = session::spec_with_status(agent, &root, cwd, repo, 24, 80, status);
    // Lifted before the spec moves into the spawn: the bridge announces the name
    // in `session-open`, which is how the shell learns it without deriving the
    // format a second time.
    let labels = SessionLabels {
        name: spec.name.clone(),
        checkout: checkout.as_ref().map(|c| c.name().to_string()),
    };
    match sessions.spawn_attached_as(
        id,
        repo.to_string(),
        agent_str.to_string(),
        "agent".to_string(),
        None,
        labels.checkout.clone(),
        spec,
    ) {
        Ok((id, att)) => ws.on_upgrade(move |socket| {
            session_ws(socket, att, id, daemon_id, environment, labels, shutdown)
        }),
        Err(e) => {
            tracing::warn!(error = %e, "failed to spawn a workbench session");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to spawn session").into_response()
        }
    }
}
