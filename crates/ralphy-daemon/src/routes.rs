//! The daemon's HTTP surface: the router, the auth guard over every route,
//! and the helpers the route handlers share. The handlers themselves live in
//! one module per URL space.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Form, Query};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::protocol::{Command, Frame};
use crate::StorePaths;
use crate::{auth, desk, fleet, identity, peer, protocol, registry, rekey, session, watch};

mod api_fleet;
mod api_read;
mod api_security;
mod api_sessions;
mod guard;
mod headers;
mod presence;
mod ws_command;
mod ws_session;
mod ws_tree;

pub(crate) use api_fleet::*;
pub(crate) use api_read::*;
pub(crate) use api_security::*;
pub(crate) use api_sessions::*;
pub(crate) use guard::*;
#[cfg(test)]
pub(crate) use headers::{content_security_policy, inline_script_bodies, script_hash};
pub(crate) use presence::*;
pub(crate) use ws_command::*;
pub(crate) use ws_session::*;
pub(crate) use ws_tree::*;

/// Capacity of the shared run-exit ring (ONE buffer for all `/ws/tree`
/// subscribers, not one each). A subscriber that falls behind it just skips
/// ahead — the browser's re-read is idempotent — so this bounds memory rather
/// than correctness.
pub(crate) const RUN_EXIT_CAP: usize = 32;

pub(crate) type AgentLocator = Arc<dyn Fn(session::Agent) -> Option<PathBuf> + Send + Sync>;

pub(crate) struct RouterDependencies {
    pub(crate) auth: Arc<auth::AuthState>,
    pub(crate) roster_locator: AgentLocator,
}

pub(crate) fn router_with_roster(
    identity: Option<identity::Identity>,
    registry_path: PathBuf,
    usage_dir: PathBuf,
    stores: StorePaths,
    start: Instant,
    shutdown: tokio::sync::watch::Receiver<bool>,
    dependencies: RouterDependencies,
) -> Router {
    let RouterDependencies {
        auth,
        roster_locator,
    } = dependencies;
    let ws_identity = identity.clone();
    // The session manager owns sessions for this router's lifetime (the tmux
    // model, issue #166). Constructed here — NOT a `router` parameter — so the
    // public `router` signature and its call sites are untouched; production
    // calls `router` exactly once, so one manager per router is correct.
    let sessions = Arc::new(session::SessionManager::new());
    // `shutdown` is consumed by the `/ws` presence closure; clone one for the
    // session route so a live session bridge also stops serving on graceful
    // shutdown (it detaches, never closing the session).
    let session_shutdown = shutdown.clone();
    let session_registry = registry_path.clone();
    // The retired `console_worktree` key (ADR-0063 §3, #408) is noticed HERE and
    // nowhere else: production builds the router once (see `sessions` above),
    // which is what makes this "logged once" without a `Once`; `load_from` and
    // the routes never log it.
    match registry::load_from(&registry_path) {
        Ok(store) => {
            for slug in store.retired_console_worktree() {
                tracing::warn!(
                    %slug,
                    "repos.toml: `console_worktree` is retired (ADR-0063 §3) — a console opens in the worktree selected in the picker; the key is ignored and dropped on the next write"
                );
            }
        }
        Err(e) => tracing::warn!(error = %e, "repo registry unreadable at startup"),
    }
    // The desk (ADR-0050) is a sibling of `repos.toml`, so it inherits the
    // `$RALPHY_DAEMON_DIR` rooting `registry_path` already resolved — same rule
    // the `sessions`/`watchers` managers follow: derived here, never a `router`
    // parameter, so the public signature and its call sites hold.
    let desk_path = registry_path.with_file_name("desk.toml");
    // The peer store (ADR-0052 §3) is a sibling directory of `repos.toml`, derived
    // the same way `desk_path` is — inside `router`, never a parameter, so the
    // public signature and its call sites hold.
    let peers_dir = registry_path.with_file_name("peers");
    let nudge_peers_dir = peers_dir.clone();
    // This daemon's own environment label, resolved once: the handshake serves it
    // so a peer's diagnosis can name WHICH machine answered.
    let peer_environment = peer::detect_environment();
    let bound_port = auth.bound_port();
    let session_host = SessionHost {
        peers_dir: peers_dir.clone(),
        identity: identity.clone(),
        environment: peer_environment.clone(),
        bound_port,
    };
    let sessions_identity = identity.clone();
    let sessions_environment = peer_environment.clone();
    let sessions_peers = peers_dir.clone();
    let close_identity = identity.clone();
    let close_peers = peers_dir.clone();
    // Captured BEFORE `identity` is moved into the `/api/identity` closure, the
    // same pattern as `command_daemon_id`.
    let hello_identity = identity.clone();
    let fleet_identity = identity.clone();
    // The last repo list each peer actually served, remembered for this router's
    // lifetime so an unreachable peer's rows stay listed (ADR-0052 §5: marked,
    // never removed). NOT a background poller and NOT persisted: it is written
    // only by a SUCCESSFUL probe inside a request, so liveness is still computed
    // fresh on every `/api/fleet` — only "last known" is remembered.
    let peer_repo_cache: PeerRepoCache =
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            String,
            fleet::PeerRepoStore,
        >::new()));
    // The `(slug, remote)` pairs `/api/repos` has already handed to the
    // registrar this router lifetime (ADR-0036 amendment 2026-09-16): a hash
    // key whose remote yields no forge slug must not respawn on every page.
    let heal_memo = rekey::heal_memo();
    // The live file-tree watcher (#196) is shared across every `/ws/tree`
    // connection for this router's lifetime — same ownership model as `sessions`,
    // constructed here (NOT a `router` param) so the `router` signature holds.
    let watchers = Arc::new(watch::WatcherManager::new(watch::MAX_WATCHES));
    let peer_watch_subs = Arc::new(fleet::watchsub::WatchSubs::new(watchers.clone()));
    // Where `/api/release` reads what the watch cached: the same sibling rooting
    // `desk.toml` uses, so a scratch store keeps its own. The watch itself is
    // spawned by `serve`, never here — a router built in a test must not reach
    // the network, nor write a cache into whatever directory it was built from.
    let release_store = registry_path.parent().map(Path::to_path_buf);
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        let weak_subs = Arc::downgrade(&peer_watch_subs);
        runtime.spawn(async move {
            let mut interval = tokio::time::interval(fleet::watchsub::IDLE_EXPIRY / 2);
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(subs) = weak_subs.upgrade() else {
                    break;
                };
                subs.sweep(fleet::watchsub::IDLE_EXPIRY);
            }
        });
    }
    // The run-completion nudge bus (#310, ADR-0036 amendment): the Spawn path
    // sends the repo slug of every dispatched child that exits, and every
    // `/ws/tree` connection relays it as `changes.dirty`. Daemon-wide and
    // subscription-free — same ownership model as `watchers`, so the public
    // `router` signature holds.
    let run_exits = tokio::sync::broadcast::channel::<String>(RUN_EXIT_CAP).0;
    let command_run_exits = run_exits.clone();
    let tree_run_exits = run_exits.clone();
    let tree_watchers = watchers.clone();
    let tree_registry = registry_path.clone();
    let tree_peers = peers_dir.clone();
    let tree_shutdown = shutdown.clone();
    // A dispatched run must survive daemon shutdown (inverse of the session
    // invariant), but the handler still watches `shutdown` to stop serving the
    // socket — it just never kills the child. Clone one for that route.
    let command_shutdown = shutdown.clone();
    let command_registry = registry_path.clone();
    let command_peers = peers_dir.clone();
    let peer_command_registry = registry_path.clone();
    // The daemon identity a dispatched child inherits as RALPHY_DAEMON_ID (#168):
    // captured here BEFORE `identity` is moved into the `/api/identity` closure.
    // Only the dispatch path passes it; session/console children get none.
    let command_daemon_id = identity.as_ref().map(|i| i.id.to_string());
    let tree_daemon_id = command_daemon_id.clone();
    // The daemon identity served on `/api/usage` responses: captured here BEFORE
    // `identity` is moved into the `/api/identity` closure (mirrors
    // `command_daemon_id` above).
    let usage_daemon_id = identity.as_ref().map(|i| i.id.to_string());
    // The nudge waits for the peer to answer, so it probes — and a probe needs to
    // know who this daemon is to refuse dialling itself. Same capture-before-move
    // pattern as `command_daemon_id`.
    let nudge_daemon_id = identity.as_ref().map(|i| i.id.to_string());
    let usage_peers = peers_dir.clone();
    // `/api/spend` reads the same three inputs `/api/usage` does, cloned before
    // either closure takes them.
    let spend_usage_dir = usage_dir.clone();
    let spend_stores = stores.clone();
    let spend_registry = registry_path.clone();
    let peer_usage_dir = usage_dir.clone();
    let peer_usage_stores = stores.clone();
    let peer_usage_registry = registry_path.clone();
    let peer_usage_daemon_id = usage_daemon_id.clone();
    let agents_daemon_id = usage_daemon_id.clone().unwrap_or_default();
    let agents_peers = peers_dir.clone();
    // The avatar the login card wears (and ONLY the avatar): captured here BEFORE
    // `identity` moves into the `/api/identity` closure. `/api/session` is
    // allowlisted pre-login, so anything added to it is readable by an
    // unauthenticated caller — see [`SessionState::avatar`] for why one glyph
    // from a fixed public pool is the whole of what this leg may carry.
    let session_avatar = identity.as_ref().map(|i| i.avatar.clone());
    // The login and security routes need the runtime auth state: to read the
    // CURRENT policy (validate a code, sign a cookie), rebuild it after a mutation,
    // and bump the session epoch. Cloned (an `Arc`) BEFORE `auth` is moved into the
    // guard layer below.
    let login_auth = auth.clone();
    let sec_auth = auth.clone();
    Router::new()
        .route("/api/identity", get(move || identity_route(identity)))
        .route(
            "/api/peer/hello",
            get({
                let id = hello_identity.clone();
                let env = peer_environment.clone();
                move || peer_hello_route(id.clone(), env.clone())
            }),
        )
        .route(
            "/api/peer/usage",
            get(move |q: Query<UsageQuery>| {
                usage_local_route(
                    peer_usage_dir.clone(),
                    peer_usage_stores.clone(),
                    peer_usage_registry.clone(),
                    peer_usage_daemon_id.clone(),
                    q.0.since,
                )
            }),
        )
        .route(
            "/api/peer/command",
            post({
                let registry = peer_command_registry.clone();
                let daemon_id = command_daemon_id.clone();
                let sessions = sessions.clone();
                move |body: Json<protocol::Command>| {
                    peer_command_route(registry.clone(), daemon_id.clone(), sessions.clone(), body)
                }
            }),
        )
        .route(
            "/api/peer/tree/poll",
            post({
                let registry = registry_path.clone();
                let subs = peer_watch_subs.clone();
                move |body: Json<PeerTreePoll>| {
                    peer_tree_poll_route(registry.clone(), subs.clone(), body)
                }
            }),
        )
        .route(
            "/api/peer/tree/close",
            post({
                let subs = peer_watch_subs.clone();
                move |body: Json<PeerTreeClose>| peer_tree_close_route(subs.clone(), body)
            }),
        )
        .route("/api/about", get(about_route))
        .route(
            "/api/release",
            get({
                let store = release_store.clone();
                move || release_route(store.clone())
            }),
        )
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
                let p = registry_path.clone();
                let memo = heal_memo.clone();
                move || repos_route(p, memo)
            }),
        )
        .route(
            "/api/fleet",
            get({
                let registry = registry_path.clone();
                let peers = peers_dir.clone();
                let id = fleet_identity.clone();
                let env = peer_environment.clone();
                let cache = peer_repo_cache.clone();
                move || {
                    fleet_route(
                        registry.clone(),
                        peers.clone(),
                        id.clone(),
                        env.clone(),
                        cache.clone(),
                        bound_port,
                    )
                }
            }),
        )
        .route(
            "/api/fleet/nudge",
            post({
                let peers = nudge_peers_dir.clone();
                let daemon_id = nudge_daemon_id.clone();
                move |q: Query<NudgeQuery>| {
                    fleet_nudge_route(peers.clone(), daemon_id.clone(), bound_port, q.0.daemon_id)
                }
            }),
        )
        .route(
            "/api/usage",
            get({
                let dir = usage_dir.clone();
                let stores = stores.clone();
                let registry = registry_path.clone();
                let daemon_id = usage_daemon_id.clone();
                let peers = usage_peers.clone();
                move |q: Query<UsageQuery>| {
                    usage_route(
                        dir,
                        stores,
                        registry,
                        peers,
                        daemon_id,
                        q.0.since,
                        q.0.project,
                        q.0.period,
                    )
                }
            }),
        )
        .route(
            "/api/spend",
            get({
                let dir = spend_usage_dir.clone();
                let stores = spend_stores.clone();
                let registry = spend_registry.clone();
                move |q: Query<SpendQuery>| {
                    spend_route(
                        dir.clone(),
                        stores.clone(),
                        registry.clone(),
                        q.0.project,
                        q.0.period,
                    )
                }
            }),
        )
        .route(
            "/ws",
            get(move |ws: WebSocketUpgrade| {
                let id = ws_identity.clone();
                let shutdown = shutdown.clone();
                async move {
                    ws.on_upgrade(move |socket| ws_presence_loop(socket, id, start, shutdown))
                }
            }),
        )
        .route(
            "/ws/session",
            get({
                let sessions = sessions.clone();
                move |ws: WebSocketUpgrade, q: Query<SessionQuery>| {
                    let sessions = sessions.clone();
                    let registry_path = session_registry.clone();
                    let shutdown = session_shutdown.clone();
                    let host = session_host.clone();
                    async move {
                        session_ws_upgrade(ws, q, sessions, registry_path, host, shutdown).await
                    }
                }
            }),
        )
        .route(
            "/api/sessions",
            get({
                let sessions = sessions.clone();
                move |Query(query): Query<SessionsQuery>| {
                    sessions_route(
                        sessions.clone(),
                        sessions_peers.clone(),
                        sessions_identity.clone(),
                        sessions_environment.clone(),
                        query.local == Some(1),
                    )
                }
            }),
        )
        .route(
            "/api/desk",
            get({
                let path = desk_path.clone();
                let registry = registry_path.clone();
                move || desk_get_route(path.clone(), registry.clone())
            })
            .put({
                let path = desk_path.clone();
                let registry = registry_path.clone();
                move |Json(up): Json<desk::DeskUpload>| {
                    desk_put_route(path.clone(), registry.clone(), up)
                }
            }),
        )
        .route(
            "/api/sessions/close",
            post({
                let sessions = sessions.clone();
                move |q: Query<CloseQuery>| {
                    close_session_route(
                        q,
                        sessions.clone(),
                        close_peers.clone(),
                        close_identity.clone(),
                    )
                }
            }),
        )
        .route(
            "/ws/command",
            get({
                let sessions = sessions.clone();
                move |ws: WebSocketUpgrade| {
                    let registry_path = command_registry.clone();
                    let shutdown = command_shutdown.clone();
                    let daemon_id = command_daemon_id.clone();
                    let run_exits = command_run_exits.clone();
                    let peers_dir = command_peers.clone();
                    let sessions = sessions.clone();
                    async move {
                        ws.on_upgrade(move |socket| {
                            command_ws(
                                socket,
                                registry_path,
                                peers_dir,
                                shutdown,
                                daemon_id,
                                run_exits,
                                bound_port,
                                sessions,
                            )
                        })
                    }
                }
            }),
        )
        .route(
            "/ws/tree",
            get(move |ws: WebSocketUpgrade| {
                let watchers = tree_watchers.clone();
                let registry_path = tree_registry.clone();
                let peers_dir = tree_peers.clone();
                let daemon_id = tree_daemon_id.clone();
                let shutdown = tree_shutdown.clone();
                let run_exits = tree_run_exits.clone();
                async move {
                    ws.on_upgrade(move |socket| {
                        tree_ws(
                            socket,
                            watchers,
                            registry_path,
                            peers_dir,
                            daemon_id,
                            shutdown,
                            run_exits,
                        )
                    })
                }
            }),
        )
        .route(
            "/api/login",
            post({
                let auth = login_auth.clone();
                move |form: Form<LoginForm>| {
                    let auth = auth.clone();
                    async move { login_submit(auth, form).await }
                }
            }),
        )
        .route(
            "/api/session",
            get({
                let auth = login_auth.clone();
                let avatar = session_avatar.clone();
                move |headers: axum::http::HeaderMap| {
                    let auth = auth.clone();
                    let avatar = avatar.clone();
                    async move { session_state_route(auth, avatar, headers).await }
                }
            }),
        )
        .route(
            "/api/logout",
            post({
                let auth = sec_auth.clone();
                move || logout_route(auth.clone())
            }),
        )
        .route("/api/security/state", get(security_state_route))
        .route(
            "/api/security/totp/enroll",
            post(security_totp_enroll_route),
        )
        .route(
            "/api/security/totp/confirm",
            post({
                let auth = sec_auth.clone();
                move |form: Form<ConfirmForm>| security_totp_confirm_route(auth.clone(), form)
            }),
        )
        .route(
            "/api/security/totp/revoke",
            post({
                let auth = sec_auth.clone();
                move |form: Form<RevokeForm>| security_totp_revoke_route(auth.clone(), form)
            }),
        )
        .route(
            "/api/security/password",
            post({
                let auth = sec_auth.clone();
                move |form: Form<PasswordForm>| security_password_route(auth.clone(), form)
            }),
        )
        .route(
            "/api/security/token/remint",
            post({
                let auth = sec_auth.clone();
                move |form: Form<RemintForm>| security_token_remint_route(auth.clone(), form)
            }),
        )
        .route(
            "/api/release/watch",
            post({
                let store = release_store.clone();
                move |form: Form<ReleaseWatchForm>| release_watch_route(store.clone(), form)
            }),
        )
        .route(
            "/api/security/require-login",
            post({
                let auth = sec_auth.clone();
                move |form: Form<RequireLoginForm>| security_require_login_route(auth.clone(), form)
            }),
        )
        .fallback(ui_asset)
        // The auth guard wraps EVERY route above — the API handlers, all three
        // WS upgrades, and the UI fallback — so a network bind rejects an
        // unauthenticated request before it reaches any handler or upgrade.
        .layer(axum::middleware::from_fn_with_state(auth, require_auth))
        // Outermost, so the security headers ride every response the guard
        // lets through AND every refusal it writes itself (audit F3).
        .layer(axum::middleware::map_response(headers::security_headers))
}

/// Seconds since the Unix epoch. A backward clock (`SystemTime` before epoch)
/// yields `0`, which only makes cookies look more expired — fail closed.
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) fn encode_query_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Send a structured command reply frame over the socket, ignoring a send error
/// (the client may already be gone).
pub(crate) async fn send_command(
    socket: &mut WebSocket,
    id: u64,
    verb: &str,
    payload: serde_json::Value,
) {
    let frame = Frame::Command(Command {
        id,
        verb: verb.to_string(),
        payload,
    });
    let _ = socket
        .send(Message::Binary(protocol::encode(&frame).into()))
        .await;
}

/// Run one blocking filesystem read off the tokio runtime, the same rule
/// `collect_config` follows for a spawn (ADR-0036 §2). The `tree` reads are
/// synchronous `read_dir`/`read` calls, and on Windows a cold directory behind a
/// virus scanner answers in tens of milliseconds — long enough that running them
/// inline parked a runtime worker and stalled every OTHER socket served by it,
/// including the ones the same click had just opened.
///
/// `None` means the blocking task did not complete (it panicked, or was
/// cancelled). That is not the same as a refused read, so the callers surface it
/// as its own reason rather than borrowing "not found", which would tell the
/// operator a file is absent when the truth is that we failed to look.
pub(crate) async fn blocking_read<T, F>(f: F) -> Option<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).await.ok()
}

/// Read the peer store off the reactor. A panic inside `read_store` degrades to
/// "no peers announced", so it is LOGGED rather than swallowed — silently
/// shorter is exactly the failure the fleet view must never show.
pub(crate) async fn read_peer_store(
    dir: PathBuf,
) -> (Vec<peer::PeerDescriptor>, Vec<peer::PeerReject>) {
    match tokio::task::spawn_blocking(move || peer::read_store(&dir)).await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!(error = %e, "reading the peer store failed; serving no peers");
            (Vec::new(), Vec::new())
        }
    }
}
