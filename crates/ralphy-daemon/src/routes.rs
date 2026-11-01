//! The daemon's HTTP surface: the router, the auth guard over every route,
//! and the helpers the route handlers share. The handlers live in one module
//! per URL space, and each of those modules registers its own routes
//! (`<area>_routes`); the router merges them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::Router;

use crate::mux::{ChannelSender, WriterClosed};
use crate::protocol::{Command, Frame};
use crate::StorePaths;
use crate::{audit, auth, fleet, identity, peer, protocol, registry, rekey, session, watch};

mod api_attention;
mod api_audit;
mod api_command;
mod api_desk;
mod api_fleet;
mod api_peer;
mod api_read;
mod api_release;
mod api_security;
mod api_sessions;
mod api_stall;
mod api_telegram;
mod api_update;
mod api_usage;
mod audit_layer;
mod guard;
mod headers;
mod presence;
mod ui_asset;
mod ws_command;
mod ws_session;
mod ws_tree;

pub(crate) use api_attention::*;
pub(crate) use api_audit::*;
pub(crate) use api_command::*;
pub(crate) use api_desk::*;
pub(crate) use api_fleet::*;
pub(crate) use api_peer::*;
pub(crate) use api_read::*;
pub(crate) use api_release::*;
pub(crate) use api_security::*;
pub(crate) use api_sessions::*;
pub(crate) use api_stall::*;
pub(crate) use api_telegram::*;
pub(crate) use api_update::*;
pub(crate) use api_usage::*;
pub(crate) use audit_layer::*;
pub(crate) use guard::*;
#[cfg(test)]
pub(crate) use headers::{content_security_policy, inline_script_bodies};
pub(crate) use presence::*;
pub(crate) use ui_asset::*;
pub(crate) use ws_command::*;
pub(crate) use ws_session::*;
pub(crate) use ws_tree::*;

/// Capacity of the shared run-exit ring (ONE buffer for all `changes`
/// channels, not one each). A subscriber that falls behind it just skips
/// ahead — the browser's re-read is idempotent — so this bounds memory rather
/// than correctness.
pub(crate) const RUN_EXIT_CAP: usize = 32;

pub(crate) type AgentLocator = Arc<dyn Fn(session::Agent) -> Option<PathBuf> + Send + Sync>;

pub(crate) struct RouterDependencies {
    pub(crate) auth: Arc<auth::AuthState>,
    pub(crate) roster_locator: AgentLocator,
    /// Tracks the page sockets, so the stop of `serve` can wait for them.
    pub(crate) page_sockets: crate::mux::Sockets,
}

/// The values the route areas share, built once per router. Each area's
/// `<area>_routes` function borrows it and clones what its routes capture.
pub(crate) struct RouterShared {
    pub(crate) identity: Option<identity::Identity>,
    /// This daemon's id as text: what a dispatched child inherits as
    /// `RALPHY_DAEMON_ID` (#168), what `/api/usage` serves, and what a probe
    /// compares against to refuse dialling itself.
    pub(crate) daemon_id: Option<String>,
    /// This daemon's environment label, resolved once: the handshake serves it
    /// so a peer's diagnosis can name WHICH machine answered.
    pub(crate) environment: String,
    /// The port this daemon bound, so a peer probe can refuse to dial itself.
    pub(crate) bound_port: u16,
    pub(crate) registry_path: PathBuf,
    pub(crate) peers_dir: PathBuf,
    pub(crate) desk_path: PathBuf,
    pub(crate) usage_dir: PathBuf,
    pub(crate) stores: StorePaths,
    /// Where `/api/release` reads what the watch cached.
    pub(crate) release_store: Option<PathBuf>,
    pub(crate) start: Instant,
    pub(crate) shutdown: tokio::sync::watch::Receiver<bool>,
    pub(crate) auth: Arc<auth::AuthState>,
    pub(crate) roster_locator: AgentLocator,
    pub(crate) sessions: Arc<session::SessionManager>,
    pub(crate) watchers: Arc<watch::WatcherManager>,
    pub(crate) peer_watch_subs: Arc<fleet::watchsub::WatchSubs>,
    pub(crate) peer_repo_cache: PeerRepoCache,
    pub(crate) heal_memo: rekey::HealMemo,
    pub(crate) run_exits: tokio::sync::broadcast::Sender<String>,
    pub(crate) pushes: tokio::sync::broadcast::Sender<Push>,
    pub(crate) audit: Arc<audit::Audit>,
    /// Whether the operator is at one of this daemon's pages: `/ws` writes
    /// it, the attention tick reads it (ADR-0085 D14).
    pub(crate) presence: crate::presence_board::Shared,
    /// The claim of a main daemon on this one: `POST /api/telegram/main`
    /// writes it, `GET /api/telegram` and the attention tick read it
    /// (ADR-0085 D14).
    pub(crate) telegram_claim: crate::telegram_state::SharedClaim,
    /// Gives each page socket the hold the stop of `serve` waits for.
    pub(crate) page_sockets: crate::mux::Sockets,
}

#[expect(
    clippy::too_many_arguments,
    reason = "the router's stores, clock, shutdown and dependencies; \
         its callers are in lib.rs and the daemon tests"
)]
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
        page_sockets,
    } = dependencies;
    // The session manager owns sessions for this router's lifetime (the tmux
    // model, issue #166). Constructed here — NOT a `router` parameter — so the
    // public `router` signature and its call sites are untouched; production
    // calls `router` exactly once, so one manager per router is correct. Its id
    // record sits beside `repos.toml`, in the store, like `desk.toml` below.
    let sessions = Arc::new(session::SessionManager::continuing(
        registry_path.with_file_name("daemon-session-id"),
    ));
    // The retired `console_worktree` key (ADR-0063 §3, #408) is noticed HERE and
    // nowhere else: production builds the router once (see `sessions` above),
    // which is what makes this "logged once" without a `Once`; `load_from` and
    // the routes never log it.
    warn_retired_console_worktree(&registry_path);
    // The desk (ADR-0050) is a sibling of `repos.toml`, so it inherits the
    // `$RALPHY_DAEMON_DIR` rooting `registry_path` already resolved — same rule
    // the `sessions`/`watchers` managers follow: derived here, never a `router`
    // parameter, so the public signature and its call sites hold.
    let desk_path = registry_path.with_file_name("desk.toml");
    // The peer store (ADR-0052 §3) is a sibling directory of `repos.toml`, derived
    // the same way `desk_path` is — inside `router`, never a parameter, so the
    // public signature and its call sites hold.
    let peers_dir = registry_path.with_file_name("peers");
    // The last repo list and environment label each peer actually served,
    // remembered for this router's lifetime so an unreachable peer's rows stay
    // listed under the same header (ADR-0052 §5: marked, never removed). NOT a
    // background poller and NOT persisted: it is written only by a SUCCESSFUL
    // probe inside a request, so liveness is still computed fresh on every
    // `/api/fleet` — only "last known" is remembered.
    let peer_repo_cache: PeerRepoCache = Arc::default();
    // The `(slug, remote)` pairs `/api/repos` has already handed to the
    // registrar this router lifetime (ADR-0036 amendment 2026-09-16): a hash
    // key whose remote yields no forge slug must not respawn on every page.
    let heal_memo = rekey::heal_memo();
    // The live file-tree watcher (#196) is shared across every tree
    // connection for this router's lifetime — same ownership model as `sessions`,
    // constructed here (NOT a `router` param) so the `router` signature holds.
    let watchers = Arc::new(watch::WatcherManager::new(watch::MAX_WATCHES));
    let peer_watch_subs = Arc::new(fleet::watchsub::WatchSubs::new(watchers.clone()));
    // Where `/api/release` reads what the watch cached: the same sibling rooting
    // `desk.toml` uses, so a scratch store keeps its own. The watch itself is
    // spawned by `serve`, never here — a router built in a test must not reach
    // the network, nor write a cache into whatever directory it was built from.
    let release_store = registry_path.parent().map(Path::to_path_buf);
    spawn_watch_sub_sweep(&peer_watch_subs);
    // The run-completion nudge bus (#310, ADR-0036 amendment): the Spawn path
    // sends the repo slug of every dispatched child that exits, and every
    // `changes` channel relays it as `changes.dirty`. Daemon-wide and
    // subscription-free — same ownership model as `watchers`, so the public
    // `router` signature holds.
    let run_exits = tokio::sync::broadcast::channel::<String>(RUN_EXIT_CAP).0;
    // The push bus of the shown facts the daemon owns (ADR-0070 D2): every
    // `/ws` relays it. Daemon-wide, like `run_exits`.
    let pushes = tokio::sync::broadcast::channel::<Push>(PUSH_CAP).0;
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(watch_stores(
            registry_path.clone(),
            peers_dir.clone(),
            pushes.clone(),
            shutdown.clone(),
        ));
    }
    // The audit log (ADR-0074) sits in the store beside `repos.toml`. A store
    // with no directory (a test's bare relative name) keeps no log. It pushes
    // `logins.dirty` when the login notices change (#717).
    let audit = Arc::new(
        audit::Audit::in_dir(
            registry_path
                .parent()
                .filter(|dir| !dir.as_os_str().is_empty())
                .map(Path::to_path_buf),
        )
        .with_pushes(pushes.clone()),
    );
    let shared = RouterShared {
        daemon_id: identity.as_ref().map(|i| i.id.to_string()),
        identity,
        environment: peer::detect_environment(),
        bound_port: auth.bound_port(),
        registry_path,
        peers_dir,
        desk_path,
        usage_dir,
        stores,
        release_store,
        start,
        shutdown,
        auth: auth.clone(),
        roster_locator,
        sessions,
        watchers,
        peer_watch_subs,
        peer_repo_cache,
        heal_memo,
        run_exits,
        pushes,
        audit: audit.clone(),
        presence: Default::default(),
        telegram_claim: Default::default(),
        page_sockets,
    };
    Router::new()
        .merge(read_routes(&shared))
        .merge(usage_routes(&shared))
        .merge(release_routes(&shared))
        .merge(peer_routes(&shared))
        .merge(fleet_routes(&shared))
        .merge(presence_routes(&shared))
        .merge(session_routes(&shared))
        .merge(attention_routes(&shared))
        .merge(desk_routes(&shared))
        .merge(command_http_routes(&shared))
        .merge(security_routes(&shared))
        .merge(audit_routes(&shared))
        .merge(stall_routes(&shared))
        .merge(telegram_routes(&shared))
        .fallback(ui_asset)
        // Inside the guard: only requests the guard let through are recorded.
        .layer(axum::middleware::from_fn_with_state(audit, audit_layer))
        // The auth guard wraps EVERY route above — the API handlers, every
        // WS upgrade, and the UI fallback — so a network bind rejects an
        // unauthenticated request before it reaches any handler or upgrade.
        .layer(axum::middleware::from_fn_with_state(
            auth.clone(),
            require_auth,
        ))
        // Outermost, so the security headers ride every response the guard
        // lets through AND every refusal it writes itself (audit F3).
        .layer(axum::middleware::map_response_with_state(
            auth,
            headers::security_headers,
        ))
}

fn warn_retired_console_worktree(registry_path: &Path) {
    match registry::load_from(registry_path) {
        Ok(store) => {
            for slug in store.retired_console_worktree() {
                tracing::warn!(
                    %slug,
                    "repos.toml: the `console_worktree` setting is no longer used and is removed the next time the file is saved; a console now opens in the worktree you pick"
                );
            }
        }
        Err(e) => tracing::warn!(error = %e, "repo registry unreadable at startup"),
    }
}

/// Expire the idle peer watch subscriptions, while any router still holds
/// them. Outside a runtime (a router built in a sync test) nothing runs.
fn spawn_watch_sub_sweep(peer_watch_subs: &Arc<fleet::watchsub::WatchSubs>) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        let weak_subs = Arc::downgrade(peer_watch_subs);
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
            #[expect(
                clippy::let_underscore_must_use,
                reason = "writing to a String cannot fail"
            )]
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// The wire bytes of one `Frame::Command`.
fn command_bytes(id: u64, verb: &str, payload: serde_json::Value) -> Vec<u8> {
    protocol::encode(&Frame::Command(Command {
        id,
        verb: verb.to_string(),
        payload,
    }))
}

/// Queue one command frame on a channel of the socket's writer. Fails when
/// the writer has ended: the socket is closed.
pub(crate) async fn send_command_on(
    out: &ChannelSender,
    id: u64,
    verb: &str,
    payload: serde_json::Value,
) -> Result<(), WriterClosed> {
    out.send(command_bytes(id, verb, payload)).await
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
