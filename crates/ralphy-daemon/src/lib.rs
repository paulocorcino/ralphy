//! Ralphy's resident daemon (docs/adr/0032): a foreground HTTP listener bound
//! to localhost, serving the embedded workbench UI. This is the tracer bullet —
//! no sessions, no command vocabulary yet — but the shape is the decided one:
//! a library crate wired by `ralphy-cli`, the workspace's async runtime (tokio +
//! axum) confined here, runs reached only by spawning `ralphy` processes (never
//! by importing the core).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Form, Query, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use include_dir::{include_dir, Dir};

pub mod auth;
pub mod autostart;
pub mod confine;
pub mod cookie;
pub mod desk;
pub mod dispatch;
pub mod epoch;
pub mod fleet;
pub mod fswrite;
pub mod identity;
pub mod password;
pub mod peer;
pub mod protocol;
pub mod registry;
pub mod roster;
pub mod session;
pub mod totp;
pub mod tree;
pub mod usage;
pub mod watch;

use protocol::{Command, Frame, Presence};

/// The daemon's default TCP port. "ralphy" on a phone keypad starts 7-2-5-7.
pub const DEFAULT_PORT: u16 = 7257;

/// The embedded workbench UI, baked in at build time like `assets/prompts` — the
/// daemon reads no files from disk at runtime (ADR-0032 §4). Promoted to the
/// daemon's `/` in #200 (PRD #185); the SPA self-gates its login (see
/// [`require_auth`]), so there is no separate server-rendered login page.
static UI: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/ui");

/// What the composition root decides; everything else is the daemon's.
pub struct DaemonConfig {
    /// TCP port for the listener.
    pub port: u16,
    /// The interface to bind. Defaults to `127.0.0.1` (loopback only); a
    /// non-localhost bind is an explicit opt-in that REQUIRES a bearer access
    /// token, enforced at boot by [`auth::AuthPolicy::for_bind`] (ADR-0032 §4).
    pub bind: IpAddr,
    /// Extra host names this daemon answers as, beyond the ones its bind implies:
    /// a MagicDNS name, a reverse-proxy hostname. The cross-site gate refuses any
    /// other `Host`, which is what keeps DNS rebinding out — so reaching the
    /// daemon by NAME (rather than by the bound IP) is an explicit declaration.
    pub allowed_hosts: Vec<String>,
    /// Directories this daemon announces itself into as a peer descriptor
    /// (ADR-0052 §3) — typically the OTHER environment's `.ralphy` store, e.g.
    /// `/mnt/c/Users/<user>/.ralphy` from inside WSL. A directory is the only
    /// thing an announcer can know about its peer; empty means "a fleet of one".
    pub peer_stores: Vec<PathBuf>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            bind: Ipv4Addr::LOCALHOST.into(),
            allowed_hosts: Vec::new(),
            peer_stores: Vec::new(),
        }
    }
}

/// Compose the bind address from an interface and port. Centralized so the
/// resolved interface flows through one place (the auth policy keys on
/// `addr.ip()`).
pub fn bind_addr(ip: IpAddr, port: u16) -> SocketAddr {
    SocketAddr::new(ip, port)
}

/// Run the daemon in the foreground until Ctrl+C. Blocking on purpose: the
/// tokio runtime is created and dropped inside, so callers (the sync CLI)
/// never see async types.
pub fn run(config: DaemonConfig) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building the daemon's tokio runtime")?;
    runtime.block_on(serve(
        bind_addr(config.bind, config.port),
        config.allowed_hosts,
        config.peer_stores,
    ))
}

async fn serve(
    addr: SocketAddr,
    allowed_hosts: Vec<String>,
    peer_stores: Vec<PathBuf>,
) -> Result<()> {
    // Captured at daemon start so every presence heartbeat reports process
    // uptime, not per-connection age.
    let start = Instant::now();
    // Fired when the operator asks the daemon to stop. Every `/ws` presence loop
    // watches this so a held-open connection cannot stall graceful shutdown.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding the daemon listener on {addr}"))?;
    // Log the *bound* address, not the requested one, so a future `port: 0`
    // (OS-assigned) still reports something a browser can open.
    let addr = listener.local_addr().context("reading the bound address")?;
    tracing::info!(%addr, "daemon listening — open http://{addr} (Ctrl+C to stop)");

    // Log a load failure rather than masking a corrupt daemon.toml as
    // "un-baptized" — the operator needs to see the real fault, not a silent
    // fall-through to no-identity.
    let id = match identity::load_current() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load daemon identity; serving without one");
            None
        }
    };
    if id.is_none() {
        tracing::info!("daemon has no identity yet — run `ralphy daemon setup` to baptize it");
    }
    // Resolve the effective access token, then the bind policy. INVARIANT:
    // `for_bind` returns Err and aborts startup on a non-loopback bind with no
    // token — the daemon must never begin serving an unauthenticated network
    // socket (ADR-0032 §4).
    let token = auth::effective_token()?;
    // Announce this daemon into every peer store (ADR-0052 §3). Done AFTER the
    // listener is bound so the descriptor carries the BOUND port — an OS-assigned
    // port (`--port 0`) is announced correctly, never the requested one — and
    // AFTER `effective_token`, so announcing REUSES whatever credential this
    // daemon already has (env override included) and mints only when there is
    // none. Minting first would displace a `RALPHY_DAEMON_TOKEN` env credential
    // and could flip a require-login daemon from `Localhost` to `Session`, which
    // AC5 ("the auth policy is unchanged") forbids.
    // INVARIANT: a store that cannot be written logs and is skipped — announcing
    // must never abort a listener that is already serving.
    if !peer_stores.is_empty() {
        announce_peer(&peer_stores, id.as_ref(), addr, token.clone());
    }
    // The live session epoch (ADR-0032 amendment §B): mixed into every cookie so a
    // bump is an instant, total logout. Persisted beside the token.
    let session_epoch = epoch::SessionEpoch::load(epoch::epoch_path()?)?;
    // Boot the runtime auth state (amendment §A/§B): validates the
    // network-bind-needs-a-token invariant, reads the on-disk seed / password /
    // require-login flag, and computes the initial policy. The policy is now
    // runtime-swappable — a security toggle rebuilds it in place, no restart.
    let auth_state = auth::AuthState::boot(addr, token, session_epoch, &allowed_hosts)?;
    // INVARIANT: strip the token from the process env on the boot path BEFORE any
    // child can be spawned, so every subsequent `dispatch`/`session` child
    // inherits a token-free env on ALL paths (mirrors RALPHY_EVENTS_TOKEN,
    // ADR-0019). The auth state already holds the effective token as its
    // signing-key fallback.
    auth::strip_token_from_env();

    // Resolve the registry path once and hand it to the router. `/api/repos`
    // reads it FRESH from disk on each request, so the resident daemon sees
    // writes made by separate `ralphy run` processes (ADR-0032).
    let registry_path = registry::repos_toml_path()?;
    let usage_dir = usage::usage_dir_path()?;
    let stores = StorePaths {
        claude_projects_dir: usage::claude_projects_dir_path()?,
        codex_dir: usage::codex_dir_path()?,
        opencode_db: usage::opencode_db_path()?,
        kimi_dir: usage::kimi_dir_path()?,
        kimi_code_dir: usage::kimi_code_dir_path()?,
        copilot_db: usage::copilot_db_path()?,
        cursor_dir: usage::cursor_dir_path()?,
        gemini_dir: usage::gemini_dir_path()?,
    };
    axum::serve(
        listener,
        router(
            id,
            registry_path,
            usage_dir,
            stores,
            start,
            shutdown_rx,
            auth_state,
        ),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        // Break every live `/ws` loop so graceful shutdown does not wait on
        // a long-lived heartbeat connection.
        let _ = shutdown_tx.send(true);
    })
    .await
    .context("serving the daemon listener")?;
    tracing::info!("daemon stopped");
    Ok(())
}

/// Build the descriptor this daemon announces. Pure: every input is a
/// parameter, so the three branches a live boot cannot easily exercise
/// (un-baptized, no WSL, an existing vs a freshly minted token) are unit-tested.
///
/// The announced address is always loopback: the peer transport is loopback-only
/// (ADR-0052 §2), and WSL2's `localhostForwarding` relay is what carries it
/// across the boundary.
fn announced_descriptor(
    id: &identity::Identity,
    port: u16,
    wsl_distro: Option<&str>,
    token: String,
) -> peer::PeerDescriptor {
    peer::PeerDescriptor {
        daemon_id: id.id.to_string(),
        name: id.name.clone(),
        avatar: id.avatar.clone(),
        address: Ipv4Addr::LOCALHOST.to_string(),
        port,
        environment: peer::environment_label(wsl_distro, std::env::consts::OS),
        token,
        protocol_version: peer::PEER_PROTOCOL_VERSION,
        // Only a daemon inside WSL can be woken by `wsl.exe`, so only it
        // advertises how.
        nudge: wsl_distro.map(|distro| peer::NudgeSpec {
            distro: distro.to_string(),
            unit: autostart::UNIT_NAME.to_string(),
        }),
    }
}

/// Write this daemon's peer descriptor into every store in `stores` (ADR-0052
/// §3). Each daemon announces its OWN token — there is no shared secret, so
/// rotating one daemon's token revokes exactly that one peer.
///
/// `token` is the ALREADY-RESOLVED effective token; one is minted here only when
/// there is none, so announcing never displaces an env credential nor changes
/// the bind policy. An un-baptized daemon has no identity to announce and is
/// skipped with a warning naming the command that fixes it.
fn announce_peer(
    stores: &[PathBuf],
    id: Option<&identity::Identity>,
    addr: SocketAddr,
    token: Option<String>,
) {
    // A daemon that does not listen on loopback cannot be reached by a peer, and
    // announcing `127.0.0.1` for it would produce a descriptor that dials a port
    // nothing answers — an `Unreachable` that never says the descriptor is wrong.
    if !addr.ip().is_loopback() && !addr.ip().is_unspecified() {
        tracing::warn!(
            bind = %addr.ip(),
            "--peer-store was given but this daemon does not listen on loopback — a peer              is reached over loopback only (ADR-0052 §2); announcing nothing"
        );
        return;
    }
    let Some(id) = id else {
        tracing::warn!(
            "--peer-store was given but this daemon is un-baptized — run `ralphy daemon setup`              to mint its identity, then restart; announcing nothing"
        );
        return;
    };
    let token = match token {
        Some(token) if !token.is_empty() => token,
        // No credential yet: mint the daemon's one and only token now. On a
        // loopback bind this changes no policy (`AuthPolicy::for_bind`), and the
        // peer needs SOMETHING to present.
        _ => match auth::token_path().and_then(|p| auth::ensure_token_at(&p)) {
            Ok((token, _minted)) => token,
            Err(e) => {
                tracing::warn!(error = %e, "could not resolve this daemon's access token; announcing nothing");
                return;
            }
        },
    };
    let distro = std::env::var("WSL_DISTRO_NAME")
        .ok()
        .filter(|d| !d.is_empty());
    let descriptor = announced_descriptor(id, addr.port(), distro.as_deref(), token);
    for store in stores {
        match peer::write_descriptor(store, &descriptor) {
            Ok(path) => tracing::info!(path = %path.display(), "announced this daemon as a peer"),
            Err(e) => {
                tracing::warn!(store = %store.display(), error = %e, "could not announce into this peer store")
            }
        }
    }
}

/// The per-vendor interactive session-store paths resolved once at daemon boot
/// and handed to the `/api/usage` scan — one `PathBuf` per vendor store. Grouped
/// so onboarding a vendor is a new field, not another positional threaded through
/// every `router` call site (#267); eight adjacent same-typed paths were also
/// transposition-prone (the compiler can't catch two swapped stores). `Default`
/// yields empty paths — a "no store" set the scans tolerate (ADR-0040 C6) — which
/// the daemon's tests use as their all-missing base.
#[derive(Clone, Default)]
pub struct StorePaths {
    pub claude_projects_dir: PathBuf,
    pub codex_dir: PathBuf,
    pub opencode_db: PathBuf,
    pub kimi_dir: PathBuf,
    pub kimi_code_dir: PathBuf,
    pub copilot_db: PathBuf,
    pub cursor_dir: PathBuf,
    pub gemini_dir: PathBuf,
}

/// Capacity of the shared run-exit ring (ONE buffer for all `/ws/tree`
/// subscribers, not one each). A subscriber that falls behind it just skips
/// ahead — the browser's re-read is idempotent — so this bounds memory rather
/// than correctness.
const RUN_EXIT_CAP: usize = 32;

/// The daemon's HTTP surface. Real routes sit *before* the embedded-UI
/// fallback. `GET /api/identity` returns the loaded identity as JSON, or 404
/// when the daemon is un-baptized, so the static page can render "avatar name"
/// at runtime (the embedded HTML bakes in no identity).
pub fn router(
    identity: Option<identity::Identity>,
    registry_path: PathBuf,
    usage_dir: PathBuf,
    stores: StorePaths,
    start: Instant,
    shutdown: tokio::sync::watch::Receiver<bool>,
    auth: Arc<auth::AuthState>,
) -> Router {
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
            registry::RegistryStore,
        >::new()));
    // The live file-tree watcher (#196) is shared across every `/ws/tree`
    // connection for this router's lifetime — same ownership model as `sessions`,
    // constructed here (NOT a `router` param) so the `router` signature holds.
    let watchers = Arc::new(watch::WatcherManager::new(watch::MAX_WATCHES));
    let peer_watch_subs = Arc::new(fleet::watchsub::WatchSubs::new(watchers.clone()));
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
            "/api/peer/command",
            post({
                let registry = peer_command_registry.clone();
                let daemon_id = command_daemon_id.clone();
                move |body: Json<protocol::Command>| {
                    peer_command_route(registry.clone(), daemon_id.clone(), body)
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
        .route("/api/agents", get(agents_route))
        .route(
            "/api/repos",
            get({
                let p = registry_path.clone();
                move || repos_route(p)
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
                    )
                }
            }),
        )
        .route(
            "/api/fleet/nudge",
            post({
                let peers = nudge_peers_dir.clone();
                move |q: Query<NudgeQuery>| fleet_nudge_route(peers.clone(), q.0.daemon_id)
            }),
        )
        .route(
            "/api/usage",
            get({
                let dir = usage_dir.clone();
                let stores = stores.clone();
                let registry = registry_path.clone();
                let daemon_id = usage_daemon_id.clone();
                move |q: Query<UsageQuery>| {
                    usage_route(dir, stores, registry, daemon_id, q.0.since)
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
                    async move { session_ws_upgrade(ws, q, sessions, registry_path, shutdown).await }
                }
            }),
        )
        .route(
            "/api/sessions",
            get({
                let sessions = sessions.clone();
                move || sessions_route(sessions.clone())
            }),
        )
        .route(
            "/api/desk",
            get({
                let path = desk_path.clone();
                move || desk_get_route(path.clone())
            })
            .put({
                let path = desk_path.clone();
                move |Json(up): Json<desk::DeskUpload>| desk_put_route(path.clone(), up)
            }),
        )
        .route(
            "/api/sessions/close",
            post({
                let sessions = sessions.clone();
                move |q: Query<CloseQuery>| close_session_route(q, sessions.clone())
            }),
        )
        .route(
            "/ws/command",
            get(move |ws: WebSocketUpgrade| {
                let registry_path = command_registry.clone();
                let shutdown = command_shutdown.clone();
                let daemon_id = command_daemon_id.clone();
                let run_exits = command_run_exits.clone();
                let peers_dir = command_peers.clone();
                async move {
                    ws.on_upgrade(move |socket| {
                        command_ws(
                            socket,
                            registry_path,
                            peers_dir,
                            shutdown,
                            daemon_id,
                            run_exits,
                        )
                    })
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
                move || security_totp_revoke_route(auth.clone())
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
                move || security_token_remint_route(auth.clone())
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
}

/// API endpoints reachable WITHOUT a session cookie under a `Session` policy —
/// the SPA's own login gate posts to these before it holds a cookie. Every other
/// `/api/*` and `/ws/*` endpoint stays gated; static UI bytes are served ungated
/// (see [`require_auth`]).
const LOGIN_ALLOWLIST: &[&str] = &["/api/login", "/api/session", "/api/logout"];

/// The guard over the whole axum surface. First asks the [`auth::AuthPolicy`]
/// (`Localhost` passes all; `Bearer`, and the machine leg of `Session`, pass a
/// correct `Bearer <token>`). Under a `Session` policy a request with no valid
/// bearer is then checked for a browser session cookie; failing that, a top-level
/// `GET` navigation serves the static SPA (which renders its own login gate) and
/// anything else is `401`. `Localhost`/`Bearer` keep the plain `401`
/// fall-through — fail closed.
async fn require_auth(
    State(state): State<Arc<auth::AuthState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // Cross-site first, BEFORE the policy: under `Localhost` the policy
    // authorizes everything, so a page the operator merely visits could otherwise
    // drive the whole surface — WS upgrades are not CORS-preflighted (RFC 6455
    // §10.2 makes the origin check the server's job) and a form POST is
    // CORS-simple. Refusing here covers every route and every upgrade at once.
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    if !state.same_origin(origin) || !state.host_allowed(host) {
        return (StatusCode::FORBIDDEN, "cross-origin request refused").into_response();
    }
    // Read the CURRENT policy: a security toggle may have swapped it since boot
    // (ADR-0032 amendment §A). The clone is cheap (`Session` is `Arc`); the lock
    // is released before any `.await`.
    let policy = state.policy();
    let header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if policy.authorizes(header) {
        return next.run(req).await;
    }
    if let auth::AuthPolicy::Session(session) = &policy {
        let cookie_header = req
            .headers()
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok());
        let now = now_unix();
        if session.cookie_valid(cookie_header, now) {
            // Idle-slide (amendment §D): re-issue the cookie with a later `exp`
            // (same `iat`, so the absolute cap holds) when activity moved it far
            // enough. The header must be owned before `req` is consumed by `next`.
            let slid = session
                .slide_cookie(cookie_header, now)
                .map(|c| cookie::set_cookie_value(&c));
            let mut resp = next.run(req).await;
            if let Some(set_cookie) = slid {
                if let Ok(v) = header::HeaderValue::from_str(&set_cookie) {
                    resp.headers_mut().append(header::SET_COOKIE, v);
                }
            }
            return resp;
        }
        let path = req.uri().path();
        if LOGIN_ALLOWLIST.contains(&path) {
            return next.run(req).await;
        }
        // The workbench shell is non-secret static bytes: a GET for any non-`/api`,
        // non-`/ws` path is served without a cookie so the SPA can render its own
        // opaque login gate. Every DATA endpoint (`/api/*` except the allowlist,
        // `/ws/*`) stays gated — the SPA can show nothing until `/api/login`
        // succeeds. API/WS/other verbs fail closed with 401.
        if req.method() == axum::http::Method::GET
            && !path.starts_with("/api")
            && !path.starts_with("/ws")
        {
            return next.run(req).await;
        }
        return (StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response()
}

/// Seconds since the Unix epoch. A backward clock (`SystemTime` before epoch)
/// yields `0`, which only makes cookies look more expired — fail closed.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Build the presence heartbeat for the loaded identity and the daemon's
/// current uptime. `None` identity → a heartbeat with no name/avatar (the
/// daemon is alive but un-baptized).
fn build_presence(identity: Option<&identity::Identity>, uptime: Duration) -> Frame {
    Frame::Presence(Presence {
        name: identity.map(|i| i.name.clone()),
        avatar: identity.map(|i| i.avatar.clone()),
        uptime_secs: uptime.as_secs(),
    })
}

/// Push a presence heartbeat to a connected client every 2s until it hangs up
/// or the daemon shuts down. The send loop MUST exit on every teardown path —
/// a `None`/`Close`/error from the client (the `recv` arm) OR a daemon shutdown
/// (the `shutdown` arm) — and drop the socket, so no task keeps sending after a
/// disconnect and a held-open connection cannot stall graceful shutdown.
async fn ws_presence_loop(
    mut socket: WebSocket,
    identity: Option<identity::Identity>,
    start: Instant,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            // Ok = the daemon signalled shutdown; Err = the sender was dropped
            // (its runtime is going away). Either way, stop serving this socket.
            _ = shutdown.changed() => break,
            _ = tick.tick() => {
                let frame = build_presence(identity.as_ref(), start.elapsed());
                if socket
                    .send(Message::Binary(protocol::encode(&frame).into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            incoming = socket.recv() => {
                // None (stream closed), a Close frame, or a recv error all end
                // the loop; the socket drops when this task returns.
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

/// Query for `/ws/session`. A NEW agent launch carries `repo` + `agent`; a NEW
/// free-console launch (issue #167) carries `console=1` and an optional `repo`
/// (home dir when absent); a REATTACH carries `id` (and optional `takeover=1`,
/// or `watch=1` for a read-only attach, issue #334). All optional so one struct
/// serves every shape; the handler dispatches on `id` first, then `console`.
#[derive(serde::Deserialize)]
struct SessionQuery {
    repo: Option<String>,
    agent: Option<String>,
    id: Option<u64>,
    takeover: Option<u32>,
    watch: Option<u32>,
    console: Option<u32>,
}

/// Query for `POST /api/sessions/close`: which session to end.
#[derive(serde::Deserialize)]
struct CloseQuery {
    id: u64,
}

/// Query for `GET /api/usage`: an optional `since` (RFC3339 UTC) lower bound.
/// Callers MUST URL-encode `+` as `%2B` — axum/`serde_urlencoded` decode a raw
/// `+` as a space, corrupting the `+00:00` offset.
#[derive(serde::Deserialize)]
struct UsageQuery {
    since: Option<String>,
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
/// - `?repo=<slug>&agent=<claude|codex|opencode>` — NEW agent launch. Rejects
///   (`400`) an unknown agent, an unreadable registry, or an unregistered slug
///   before upgrading; a spawn failure is `500`.
/// - `?console=1[&repo=<slug>]` — NEW free-console launch (issue #167): the
///   platform shell in the chosen repo's dir, or the home dir when `repo` is
///   absent. Rejects (`400`) an unreadable registry or an unregistered slug;
///   a spawn failure is `500`.
async fn session_ws_upgrade(
    ws: WebSocketUpgrade,
    Query(query): Query<SessionQuery>,
    sessions: Arc<session::SessionManager>,
    registry_path: PathBuf,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Response {
    if let Some(id) = query.id {
        // A watcher never touches the writer slot, so it is dispatched BEFORE the
        // attach branch and can never produce a `409`.
        if query.watch == Some(1) {
            return match sessions.watch(id) {
                Ok(att) => ws.on_upgrade(move |socket| session_ws(socket, att, id, shutdown)),
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
            Ok(att) => ws.on_upgrade(move |socket| session_ws(socket, att, id, shutdown)),
            Err(session::AttachError::Unknown) => {
                (StatusCode::NOT_FOUND, "unknown session").into_response()
            }
            Err(session::AttachError::Busy) => {
                (StatusCode::CONFLICT, "session busy").into_response()
            }
        };
    }
    if query.console == Some(1) {
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
            spec,
        ) {
            Ok((id, att)) => ws.on_upgrade(move |socket| session_ws(socket, att, id, shutdown)),
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
    // ADR-0042 D6: an ordinary Cursor run uploads the enclosing repository. The
    // run path is gated in the adapter, but this interactive launch spawns
    // `cursor-agent` directly — so the gate has to run here too, BEFORE the spec
    // is built and anything is spawned: it writes `.cursorindexingignore` into the
    // unprotected repo (announced on the daemon log) and then proceeds. A write
    // failure (read-only tree) is the only way it stops the launch.
    if agent == session::Agent::Cursor {
        let root = Path::new(&entry.path);
        if let Err(e) =
            ralphy_proc_util::cursor::indexing_gate(root, session::cursor_indexing_allowed(root))
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
    if agent == session::Agent::Gemini
        && !session::gemini_policy_path(Path::new(&entry.path)).is_file()
    {
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
    let spec = session::spec_for(agent, PathBuf::from(&entry.path), 24, 80);
    match sessions.spawn_attached(
        repo.to_string(),
        agent_str.to_string(),
        "agent".to_string(),
        spec,
    ) {
        Ok((id, att)) => ws.on_upgrade(move |socket| session_ws(socket, att, id, shutdown)),
        Err(e) => {
            tracing::warn!(error = %e, "failed to spawn a workbench session");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to spawn session").into_response()
        }
    }
}

/// Bridge one WebSocket to one daemon-owned session (the tmux model, #166).
/// FIRST replays the scrollback snapshot, then loops: session output (via the
/// broadcast `rx`) → `Frame::Terminal`; client `Frame::Terminal` → PTY stdin;
/// client `Frame::Command{verb:"resize"}` → PTY resize. The loop breaks on client
/// close/error, a send failure, an eviction (a `takeover` reattach OR the child
/// exiting), or daemon shutdown.
///
/// TEARDOWN INVARIANT (INVERTED vs #162): on EVERY exit path the bridge drops
/// `attach` — releasing the single-writer slot — and does NOT close the session.
/// A WebSocket drop detaches; the child survives it and a later reattach resumes
/// it. A session ends only via `POST /api/sessions/close` or its child exiting,
/// never because a browser tab closed.
async fn session_ws(
    mut socket: WebSocket,
    mut attach: session::Attachment,
    id: session::SessionId,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    // Register the eviction waiter BEFORE the first await. Pin ONE `notified`
    // future across the whole loop (a fresh `notified()` per iteration could miss
    // an eviction firing mid-iteration), and `enable()` it up front so the waiter
    // is parked before the snapshot replay below: an eviction (a `takeover`
    // reattach or the child exiting) that fires during that replay `await` — or in
    // the `on_upgrade` scheduling gap — is delivered as a stored permit and breaks
    // the loop on the first poll, never lost. Missing this leaks the single-writer
    // slot AND hangs the bridge forever (the `Attachment` keeps `tx` alive, so
    // `rx.recv()` never returns `Closed`).
    let evict = attach.evict.clone();
    let notified = evict.notify.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();

    // Replay the backlog first so a reattaching client sees history before the
    // live stream resumes. Skip an empty snapshot (a fresh session).
    if !attach.snapshot.is_empty() {
        let frame = Frame::Terminal {
            session: id,
            data: std::mem::take(&mut attach.snapshot),
        };
        if socket
            .send(Message::Binary(protocol::encode(&frame).into()))
            .await
            .is_err()
        {
            return;
        }
    }
    // Keep the socket warm on quiet/low-quality links: an idle terminal sends no
    // bytes, so a NAT/proxy idle-timeout (or a lossy path with nothing to
    // retransmit) silently drops it. A periodic WS ping — which the browser
    // auto-pongs — keeps intermediaries alive and surfaces a truly dead peer as a
    // send error that tears the bridge down (detach-only; the child survives for a
    // reattach). 20s is well under common 30–60s proxy idle windows.
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ping.tick().await; // consume the immediate first tick — no ping on connect

    // A DELIBERATE end (daemon shutdown, takeover/child-exit eviction, or the
    // broadcast sender closing) is ANNOUNCED after the loop — a data frame naming
    // the reason, then the Close frame. `None` means the loop fell out some other
    // way (client close, network drop, write failure) and the bridge stays SILENT:
    // announcing there would tell a client its session ended when it did not, and
    // the client would park instead of recovering the flaky link (issue #334).
    let mut end: Option<session::EndReason> = None;
    loop {
        tokio::select! {
            _ = shutdown.changed() => { end = Some(session::EndReason::DaemonShutdown); break; }
            // Taken over, closed, or the child exited — the token carries which.
            _ = &mut notified => {
                end = Some(evict.reason().unwrap_or(session::EndReason::ChildExited));
                break;
            }
            _ = ping.tick() => {
                if socket.send(Message::Ping(Default::default())).await.is_err() {
                    break;
                }
            }
            recv = attach.rx.recv() => match recv {
                Ok(bytes) => {
                    let frame = Frame::Terminal { session: id, data: bytes };
                    if socket
                        .send(Message::Binary(protocol::encode(&frame).into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                // A burst outran this slow attach; scrollback already replayed and
                // xterm.js tolerates a gap, so keep streaming.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    end = Some(session::EndReason::ChildExited);
                    break;
                }
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Binary(bytes))) => match protocol::decode(&bytes) {
                    Ok(Frame::Terminal { data, .. }) => {
                        if attach.write(&data).is_err() {
                            break;
                        }
                    }
                    Ok(Frame::Command(cmd)) if cmd.verb == "resize" => {
                        // `try_into` rejects a garbage/oversized dimension rather
                        // than truncating it into a wrong terminal size.
                        let rows: Option<u16> =
                            cmd.payload.get("rows").and_then(|v| v.as_u64()?.try_into().ok());
                        let cols: Option<u16> =
                            cmd.payload.get("cols").and_then(|v| v.as_u64()?.try_into().ok());
                        if let (Some(rows), Some(cols)) = (rows, cols) {
                            let _ = attach.resize(rows, cols);
                        }
                    }
                    _ => {} // other frames carry no session meaning here
                },
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {} // text/ping/pong: ignore
                Some(Err(_)) => break,
            },
        }
    }
    // ANNOUNCEMENT-BEFORE-CLOSE INVARIANT (issue #334), to hold on every return
    // path: a deliberate end is named in a DATA frame first, and only then does
    // the Close frame follow and the attachment drop. Meaning placed in the close
    // metadata is meaning lost — the browser reports `1005 / wasClean=false` for
    // this very `Close(None)`, which is why the client could not tell an eviction
    // from a flaky link and stole the session back. The early `return` in the
    // snapshot replay above announces nothing because the socket is already gone,
    // and `end == None` stays silent by design (see the declaration).
    // Bounded: these are the only sends made AFTER the loop that would surface a
    // wedged peer as an error, so an unbounded await here would defer
    // `drop(attach)` — and the session's scrollback ring with it — indefinitely.
    if let Some(reason) = end {
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            send_command(
                &mut socket,
                0,
                "session-end",
                serde_json::json!({ "reason": reason.as_wire() }),
            )
            .await;
            let _ = socket.send(Message::Close(None)).await;
        })
        .await;
    }
    // Detach, do NOT close: dropping `attach` releases the single-writer slot; the
    // session (and its child) live on for a later reattach.
    drop(attach);
}

/// Send a structured command reply frame over the socket, ignoring a send error
/// (the client may already be gone).
async fn send_command(socket: &mut WebSocket, id: u64, verb: &str, payload: serde_json::Value) {
    let frame = Frame::Command(Command {
        id,
        verb: verb.to_string(),
        payload,
    });
    let _ = socket
        .send(Message::Binary(protocol::encode(&frame).into()))
        .await;
}

/// Spawn-and-COLLECT a config CLI invocation (`config get|set|unset`) for a
/// Query/Mutate verb off the tokio runtime (ADR-0036 §2): unlike the streaming
/// Spawn path, a config verb yields ONE collected reply. `None` when the blocking
/// join or the spawn itself failed. Runs in `cwd` with the dispatch `daemon_id`.
async fn collect_config(
    argv: Vec<String>,
    cwd: PathBuf,
    daemon_id: Option<String>,
) -> Option<(Option<i32>, Vec<u8>)> {
    tokio::task::spawn_blocking(move || {
        let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        dispatch::collect(
            &dispatch::ProcessSpawner,
            &dispatch::ralphy_exe(),
            &argv_refs,
            &cwd,
            daemon_id.as_deref(),
        )
    })
    .await
    .ok()
    .and_then(Result::ok)
}

async fn execute_oneshot(
    verb: dispatch::Verb,
    cmd: &protocol::Command,
    repo_path: &Path,
    daemon_id: Option<&str>,
) -> Option<serde_json::Value> {
    match verb.effect_class() {
        dispatch::EffectClass::Observe => {
            let rel = cmd
                .payload
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Some(match verb {
                dispatch::Verb::TreeList => match tree::list(repo_path, rel) {
                    Ok(entries) => serde_json::json!({ "status": "ok", "entries": entries }),
                    Err(_) => serde_json::json!({ "status": "error", "reason": "not found" }),
                },
                dispatch::Verb::FileRead => match tree::read(repo_path, rel) {
                    Ok(content) => serde_json::json!({ "status": "ok", "content": content }),
                    Err(e) => serde_json::json!({ "status": "error", "reason": e.reason() }),
                },
                dispatch::Verb::ImageRead => match tree::read_image(repo_path, rel) {
                    Ok(image) => serde_json::json!({
                        "status": "ok",
                        "mediaType": image.media_type,
                        "base64": data_encoding::BASE64.encode(&image.bytes),
                    }),
                    Err(e) => serde_json::json!({ "status": "error", "reason": e.reason() }),
                },
                dispatch::Verb::RunsList => {
                    let listing =
                        ralphy_run_snapshot::list_runs(repo_path, ralphy_proc_util::pid_is_alive);
                    serde_json::json!({
                        "status": "ok",
                        "runs": listing.live,
                        "unreadable": listing.unreadable,
                    })
                }
                _ => serde_json::json!({ "status": "error", "reason": "refused" }),
            })
        }
        dispatch::EffectClass::Write => {
            let rel = cmd
                .payload
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let result = match verb {
                dispatch::Verb::FileWrite => {
                    let content = cmd
                        .payload
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    fswrite::write(repo_path, rel, content)
                }
                dispatch::Verb::FileCreate => {
                    let dir = cmd
                        .payload
                        .get("dir")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    fswrite::create(repo_path, rel, dir)
                }
                dispatch::Verb::FileRename => {
                    let to = cmd.payload.get("to").and_then(|v| v.as_str()).unwrap_or("");
                    fswrite::rename(repo_path, rel, to)
                }
                dispatch::Verb::FileDelete => fswrite::delete(repo_path, rel),
                dispatch::Verb::PlanDiscard => fswrite::discard_plan(repo_path),
                _ => Err(fswrite::WriteError::Io),
            };
            Some(match result {
                Ok(()) => serde_json::json!({ "status": "ok" }),
                Err(e) => {
                    let reason = match e {
                        fswrite::WriteError::Confined => "refused",
                        fswrite::WriteError::Conflict => "exists",
                        fswrite::WriteError::NotFound => "not found",
                        fswrite::WriteError::Io => "io error",
                    };
                    serde_json::json!({ "status": "error", "reason": reason })
                }
            })
        }
        dispatch::EffectClass::Query => {
            let (argv_result, field): (Result<Vec<String>, dispatch::ArgvError>, &str) = match verb
            {
                dispatch::Verb::ConfigGet => (dispatch::config_argv(verb, &cmd.payload), "config"),
                dispatch::Verb::BoardList => (Ok(dispatch::board_argv()), "board"),
                dispatch::Verb::IssueShow => (dispatch::issue_show_argv(&cmd.payload), "issue"),
                dispatch::Verb::BranchList => (Ok(dispatch::branch_list_argv()), "branches"),
                dispatch::Verb::ChangesList => (Ok(dispatch::changes_list_argv()), "changes"),
                dispatch::Verb::BlobRead => (dispatch::blob_read_argv(&cmd.payload), "blob"),
                dispatch::Verb::SyncStatus => (Ok(dispatch::sync_status_argv()), "sync"),
                _ => (Err(dispatch::ArgvError::BadParam("verb")), "config"),
            };
            Some(match argv_result {
                Err(e) => {
                    tracing::warn!(error = %e, "refused a query with invalid params");
                    serde_json::json!({ "status": "error", "message": "invalid query options" })
                }
                Ok(argv) => {
                    match collect_config(
                        argv,
                        repo_path.to_path_buf(),
                        daemon_id.map(str::to_owned),
                    )
                    .await
                    {
                        Some((Some(0), bytes)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            let parsed: serde_json::Value = serde_json::from_str(text.trim())
                                .unwrap_or_else(|_| {
                                    serde_json::Value::String(text.trim().to_string())
                                });
                            let mut obj = serde_json::Map::new();
                            obj.insert("status".to_string(), serde_json::json!("ok"));
                            obj.insert(field.to_string(), parsed);
                            serde_json::Value::Object(obj)
                        }
                        Some((_, bytes)) => serde_json::json!({
                            "status": "error",
                            "message": String::from_utf8_lossy(&bytes).trim(),
                        }),
                        None => {
                            serde_json::json!({ "status": "error", "message": "query read failed" })
                        }
                    }
                }
            })
        }
        dispatch::EffectClass::Mutate => {
            let argv_result = match verb {
                dispatch::Verb::ConfigSet | dispatch::Verb::ConfigUnset => {
                    dispatch::config_argv(verb, &cmd.payload)
                }
                dispatch::Verb::BranchSwitch | dispatch::Verb::BranchCreate => {
                    dispatch::branch_argv(verb, &cmd.payload)
                }
                dispatch::Verb::LabelSet => dispatch::label_argv(&cmd.payload),
                dispatch::Verb::SyncFetch | dispatch::Verb::SyncPull | dispatch::Verb::SyncPush => {
                    dispatch::sync_argv(verb)
                }
                dispatch::Verb::ChangesStage
                | dispatch::Verb::ChangesUnstage
                | dispatch::Verb::ChangesDiscard => {
                    dispatch::changes_paths_argv(verb, &cmd.payload)
                }
                dispatch::Verb::ChangesCommit => dispatch::changes_commit_argv(&cmd.payload),
                dispatch::Verb::RunStop => dispatch::run_stop_argv(&cmd.payload),
                _ => Err(dispatch::ArgvError::BadParam("verb")),
            };
            Some(match argv_result {
                Err(e) => {
                    tracing::warn!(error = %e, "refused a mutation with invalid params");
                    serde_json::json!({ "status": "error", "message": "invalid mutation options" })
                }
                Ok(argv) => {
                    match collect_config(
                        argv,
                        repo_path.to_path_buf(),
                        daemon_id.map(str::to_owned),
                    )
                    .await
                    {
                        Some((Some(0), _)) => serde_json::json!({ "status": "ok" }),
                        Some((_, bytes)) => {
                            let msg = String::from_utf8_lossy(&bytes);
                            let msg = msg.trim();
                            let msg = if msg.is_empty() { "refused" } else { msg };
                            serde_json::json!({ "status": "error", "message": msg })
                        }
                        None => serde_json::json!({
                            "status": "error",
                            "message": "mutation write failed"
                        }),
                    }
                }
            })
        }
        dispatch::EffectClass::Spawn | dispatch::EffectClass::Native => None,
    }
}

/// `GET /ws/command`: one remote command per connection. Read the first frame; a
/// `Frame::Command{verb}` naming a blessed [`dispatch::Verb`] for a registered
/// repo spawns the run and reports its lifecycle — an ack (`status:"spawned"` +
/// pid), a stream of live output (`status:"output"` + `chunk`, issue #180), then
/// the child's exit (`status:"exited"` + code). An unknown verb or an unregistered
/// repo gets one `status:"error"` frame and spawns nothing.
///
/// TEARDOWN INVARIANT (the INVERSE of `session_ws`): the dispatched run keeps its
/// OWN lifecycle. NONE of the `select!` arms — daemon shutdown, client
/// close/error, output, wait-complete — kills the child; the
/// `Box<dyn dispatch::Child>` has no kill and dropping it does not kill (std
/// semantics). A daemon shutdown or a browser disconnect stops us serving THIS
/// socket but never the run (PRD #157 story 18/20). Do not add a kill to any arm.
/// The output DRAIN task is likewise detached: it reads the child's pipe to EOF
/// regardless of client presence, so a disconnect never stalls the child on a
/// full pipe. Do not await it on a teardown arm.
async fn command_ws(
    mut socket: WebSocket,
    registry_path: PathBuf,
    peers_dir: PathBuf,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    daemon_id: Option<String>,
    run_exits: tokio::sync::broadcast::Sender<String>,
) {
    // First frame or nothing: a client that opens and hangs up spawns nothing.
    let Some(Ok(Message::Binary(bytes))) = socket.recv().await else {
        return;
    };
    let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) else {
        return;
    };
    let id = cmd.id;

    let Some(verb) = dispatch::Verb::from_query(&cmd.verb) else {
        send_command(
            &mut socket,
            id,
            &cmd.verb,
            serde_json::json!({ "status": "error", "message": "unknown verb" }),
        )
        .await;
        return;
    };
    let repo_ref = cmd
        .payload
        .get("repo")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let (descriptors, _) = read_peer_store(peers_dir).await;
    let slug = match fleet::route(repo_ref, daemon_id.as_deref().unwrap_or(""), &descriptors) {
        fleet::Route::Local { slug } => slug.to_string(),
        fleet::Route::UnknownDaemon { daemon_id } => {
            send_command(
                &mut socket,
                id,
                &cmd.verb,
                serde_json::json!({
                    "status": "error",
                    "message": format!(
                        "no environment is announced as {daemon_id} — its daemon has not written a peer descriptor into this one's store"
                    ),
                }),
            )
            .await;
            return;
        }
        fleet::Route::Peer { peer, slug } => {
            if verb.effect_class() == dispatch::EffectClass::Spawn {
                send_command(
                    &mut socket,
                    id,
                    &cmd.verb,
                    serde_json::json!({
                        "status": "error",
                        "message": format!(
                            "starting a run in {} is not federated yet",
                            peer.environment
                        ),
                    }),
                )
                .await;
                return;
            }
            let mut proxied = cmd.clone();
            proxied.payload["repo"] = serde_json::Value::String(slug.to_string());
            let body = serde_json::to_value(&proxied).expect("Command always serializes");
            let payload = match peer::client::post_json(peer, "/api/peer/command", &body).await {
                Ok((200, body)) => serde_json::from_slice(&body).unwrap_or_else(|_| {
                    serde_json::json!({
                        "status": "error",
                        "message": fleet::peer_unreachable(
                            peer,
                            "the peer answered invalid repo command data"
                        ),
                    })
                }),
                Ok((code, _)) => serde_json::json!({
                    "status": "error",
                    "message": fleet::peer_unreachable(
                        peer,
                        &format!("the peer answered HTTP {code} to a repo command")
                    ),
                }),
                Err(e) => serde_json::json!({
                    "status": "error",
                    "message": fleet::peer_unreachable(peer, &format!("{e:#}")),
                }),
            };
            send_command(&mut socket, id, &cmd.verb, payload).await;
            return;
        }
    };
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry for a command");
            send_command(
                &mut socket,
                id,
                &cmd.verb,
                serde_json::json!({ "status": "error", "message": "repo registry unreadable" }),
            )
            .await;
            return;
        }
    };
    let Some(entry) = store.entry(&slug) else {
        send_command(
            &mut socket,
            id,
            &cmd.verb,
            serde_json::json!({ "status": "error", "message": "unknown repo" }),
        )
        .await;
        return;
    };

    if let Some(payload) =
        execute_oneshot(verb, &cmd, Path::new(&entry.path), daemon_id.as_deref()).await
    {
        send_command(&mut socket, id, &cmd.verb, payload).await;
        return;
    }

    // Compose the argv from the verb + closed-enum params (ADR-0036 §1). A
    // malformed/out-of-enum param refuses the run: one error frame, no spawn.
    let argv = match dispatch::spawn_argv(verb, &cmd.payload) {
        Ok(argv) => argv,
        Err(e) => {
            tracing::warn!(error = %e, "refused a run with invalid params");
            send_command(
                &mut socket,
                id,
                &cmd.verb,
                serde_json::json!({ "status": "error", "message": "invalid run options" }),
            )
            .await;
            return;
        }
    };
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let mut child = match dispatch::dispatch(
        &dispatch::ProcessSpawner,
        &dispatch::ralphy_exe(),
        &argv_refs,
        Path::new(&entry.path),
        daemon_id.as_deref(),
    ) {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!(error = %e, "failed to spawn a dispatched command");
            send_command(
                &mut socket,
                id,
                &cmd.verb,
                serde_json::json!({ "status": "error", "message": "spawn failed" }),
            )
            .await;
            return;
        }
    };
    let pid = child.pid();
    // Take the merged output reader BEFORE `child` moves into the wait task, so
    // the drain and the wait run concurrently (each owns its half).
    let output = child.take_output();
    send_command(
        &mut socket,
        id,
        &cmd.verb,
        serde_json::json!({ "status": "spawned", "pid": pid }),
    )
    .await;

    // A DETACHED drain owns the reader and reads to EOF unconditionally — never
    // awaited on a teardown arm, so a client disconnect never stops it and the
    // child never stalls on a full pipe (see dispatch.rs OUTPUT STREAMING). A
    // dropped receiver only makes `send` error, which the drain IGNORES.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    if let Some(mut reader) = output {
        tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        let _ = tx.send(buf[..n].to_vec());
                    }
                }
            }
        });
    }

    // `Child::wait` is blocking and must not sit on the tokio runtime.
    //
    // The run-completion nudge (#310) is sent HERE, inside the blocking task, and
    // not from the wait arm below: the shutdown and client-disconnect arms `break`
    // without ever polling that arm while the run keeps living (the teardown
    // invariant above), and tokio never cancels a blocking task — so this is the
    // only site that fires on every exit path the daemon outlives (a run that
    // outlives the process has no nudge to send — the browser's reconnect
    // catch-up read covers that one). A send with no `/ws/tree`
    // subscriber is `Err`, and a nudge nobody hears is a no-op (as in `watch.rs`).
    let nudge_slug = slug.to_string();
    let mut wait = tokio::task::spawn_blocking(move || {
        let result = child.wait();
        let _ = run_exits.send(nudge_slug);
        result
    });
    // Disables the output arm once the drain channel closes (child pipe EOF), so
    // a closed `rx` never busy-loops and the other arms keep being polled.
    let mut output_open = true;
    loop {
        tokio::select! {
            // Daemon shutdown: stop serving this socket, but LEAVE the run alive.
            _ = shutdown.changed() => break,
            // Client closed or errored: same — abandon the wait, never kill.
            incoming = socket.recv() => {
                let _ = incoming;
                break;
            }
            // A live output chunk: forward it into the UI log pane.
            chunk = rx.recv(), if output_open => {
                match chunk {
                    Some(chunk) => {
                        send_command(
                            &mut socket,
                            id,
                            &cmd.verb,
                            serde_json::json!({
                                "status": "output",
                                "chunk": String::from_utf8_lossy(&chunk),
                            }),
                        )
                        .await;
                    }
                    // Drain closed (child pipe EOF): stop polling this arm and let
                    // the wait arm report the exit.
                    None => output_open = false,
                }
            }
            // The run exited: flush remaining output before the exit frame.
            // `recv().await` (not `try_recv`) closes the trailing-output race —
            // `wait` returns before the drain thread has forwarded the child's
            // final bytes. But the drain reaches EOF (and drops `tx`) only when
            // EVERY pipe write end is closed, and a `ralphy run` DESCENDANT can
            // inherit the merged fds and outlive the primary child — so we bound
            // the wait for each next chunk: an idle gap (or channel close) ends
            // the flush and we always emit `exited`, never wedging the handler.
            joined = &mut wait => {
                while let Ok(Some(chunk)) =
                    tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
                {
                    send_command(
                        &mut socket,
                        id,
                        &cmd.verb,
                        serde_json::json!({
                            "status": "output",
                            "chunk": String::from_utf8_lossy(&chunk),
                        }),
                    )
                    .await;
                }
                let code = joined.ok().and_then(|r| r.ok()).flatten();
                send_command(
                    &mut socket,
                    id,
                    &cmd.verb,
                    serde_json::json!({ "status": "exited", "code": code }),
                )
                .await;
                break;
            }
        }
    }
}

/// `GET /ws/tree`: the persistent live-tree subscription socket (#196, ADR-0036
/// §4). A client `Frame::Command{verb:"watch", payload:{repo,path}}` starts
/// watching that repo dir (subscribing to the repo's nudge broadcast on the first
/// watch); `verb:"unwatch"` releases it. A settled change on a watched dir is
/// pushed back as `Frame::Command{verb:"tree.dirty", payload:{repo,path}}`, and
/// the browser re-reads that subtree via the Observe `tree.list` path.
///
/// The socket carries a SECOND subscription kind (#300, ADR-0047 §9): `runs.watch`
/// / `runs.unwatch` hold [`watch::RUNSTATE_REL`], the repo's run-snapshot dir, and
/// a change there pushes `runs.dirty {repo}` — the browser re-reads `runs.list`.
/// Both kinds live in the SAME `watched` list, so the teardown below releases them
/// identically.
///
/// A THIRD push kind (#310, ADR-0036 amendment) is NOT watcher-fed: `changes.dirty
/// {repo}` relays the daemon-wide run-exit broadcast, so it has no subscription
/// verb, no entry in `watched`, and nothing to release — the browser filters it by
/// the repo it has open.
///
/// TEARDOWN INVARIANT: on EVERY exit path — daemon shutdown OR client close/error
/// — the connection releases EVERY dir it watched (tracked in `watched`) so the
/// last release tears the repo watcher down, and aborts its forwarder tasks. A
/// leaked watch would keep an OS watcher (and its debouncer thread) alive forever.
struct PeerTreePoller {
    task: tokio::task::JoinHandle<()>,
    paths: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    runs: Arc<std::sync::atomic::AtomicBool>,
    peer: peer::PeerDescriptor,
    sub: String,
}

fn spawn_peer_tree_poller(
    peer: peer::PeerDescriptor,
    repo_ref: String,
    slug: String,
    sub: String,
    paths: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    runs: Arc<std::sync::atomic::AtomicBool>,
    nudge_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let paths_snapshot: Vec<String> = paths
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .cloned()
                .collect();
            let body = serde_json::json!({
                "sub": sub,
                "repo": slug,
                "paths": paths_snapshot,
                "runs": runs.load(std::sync::atomic::Ordering::Relaxed),
                "timeout_ms": 25_000,
            });
            match peer::client::post_json_timeout(
                &peer,
                "/api/peer/tree/poll",
                &body,
                Duration::from_secs(27),
            )
            .await
            {
                Ok((200, bytes)) => {
                    let dirty = serde_json::from_slice::<serde_json::Value>(&bytes)
                        .ok()
                        .and_then(|value| value["dirty"].as_array().cloned())
                        .unwrap_or_default();
                    for item in dirty {
                        if let Some(path) = item["path"].as_str() {
                            if nudge_tx.send((repo_ref.clone(), path.to_string())).is_err() {
                                return;
                            }
                        }
                    }
                }
                Ok(_) | Err(_) => tokio::time::sleep(Duration::from_secs(3)).await,
            }
        }
    })
}

fn close_peer_tree_poller(poller: PeerTreePoller) {
    poller.task.abort();
    tokio::spawn(async move {
        let body = serde_json::json!({ "sub": poller.sub });
        let _ = peer::client::post_json(&poller.peer, "/api/peer/tree/close", &body).await;
    });
}

async fn tree_ws(
    mut socket: WebSocket,
    watchers: Arc<watch::WatcherManager>,
    registry_path: PathBuf,
    peers_dir: PathBuf,
    daemon_id: Option<String>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    run_exits: tokio::sync::broadcast::Sender<String>,
) {
    // Fan-in: one forwarder task per subscribed repo pipes that repo's broadcast
    // into this shared channel, so the select! loop watches ONE receiver regardless
    // of how many repos/dirs the connection holds. Keyed by repo so it is torn down
    // when this connection releases the repo's LAST dir — and re-spawned (on the
    // fresh broadcast the manager rebuilds) if the same repo is watched again.
    let (nudge_tx, mut nudge_rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    let mut forwarders: std::collections::BTreeMap<String, tokio::task::JoinHandle<()>> =
        std::collections::BTreeMap::new();
    let mut peer_pollers: std::collections::BTreeMap<String, PeerTreePoller> =
        std::collections::BTreeMap::new();
    // The (repo, rel) dirs THIS connection holds, in normalized form — held at most
    // once each (a duplicate `watch` is a no-op, so the manager refcount this
    // connection contributes stays 1 per dir and teardown releases it exactly once).
    // Doubles as the per-connection push filter (a repo's broadcast carries every
    // dir, including ones other connections watch).
    let mut watched: Vec<(String, String)> = Vec::new();
    // The run-completion nudge bus (#310): daemon-wide, held by NO watch, so it
    // needs no subscription verb and adds nothing to the teardown below. Every
    // connection relays every nudge; the browser filters by its open repo.
    let mut run_exits_rx = run_exits.subscribe();
    let mut exits_open = true;

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Binary(bytes))) => {
                    let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) else {
                        continue;
                    };
                    let repo_ref = cmd
                        .payload
                        .get("repo")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let rel = watch::norm_rel(
                        cmd.payload.get("path").and_then(|v| v.as_str()).unwrap_or(""),
                    );
                    match cmd.verb.as_str() {
                        "watch" | "runs.watch" => {
                            if repo_ref.is_empty() {
                                continue;
                            }
                            // The runs subscription ignores the payload path: its dir is
                            // fixed (ADR-0047 §9), so a client cannot aim it elsewhere.
                            let runs = cmd.verb == "runs.watch";
                            let rel = if runs { watch::RUNSTATE_REL.to_string() } else { rel };
                            let (descriptors, _) = read_peer_store(peers_dir.clone()).await;
                            let repo = match fleet::route(
                                &repo_ref,
                                daemon_id.as_deref().unwrap_or(""),
                                &descriptors,
                            ) {
                                fleet::Route::Local { slug } => slug.to_string(),
                                fleet::Route::UnknownDaemon { .. } => continue,
                                fleet::Route::Peer { peer, slug } => {
                                    let key = (repo_ref.clone(), rel.clone());
                                    if watched.contains(&key) {
                                        continue;
                                    }
                                    let poller = peer_pollers.entry(repo_ref.clone()).or_insert_with(|| {
                                        let paths = Arc::new(std::sync::Mutex::new(
                                            std::collections::BTreeSet::new(),
                                        ));
                                        let runs_flag = Arc::new(std::sync::atomic::AtomicBool::new(runs));
                                        let sub = format!(
                                            "{}-{}",
                                            daemon_id.as_deref().unwrap_or("daemon"),
                                            ulid::Ulid::new()
                                        );
                                        let task = spawn_peer_tree_poller(
                                            peer.clone(),
                                            repo_ref.clone(),
                                            slug.to_string(),
                                            sub.clone(),
                                            paths.clone(),
                                            runs_flag.clone(),
                                            nudge_tx.clone(),
                                        );
                                        PeerTreePoller {
                                            task,
                                            paths,
                                            runs: runs_flag,
                                            peer: peer.clone(),
                                            sub,
                                        }
                                    });
                                    poller
                                        .paths
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .insert(rel.clone());
                                    if runs {
                                        poller
                                            .runs
                                            .store(true, std::sync::atomic::Ordering::Relaxed);
                                    }
                                    watched.push(key);
                                    continue;
                                }
                            };
                            // Idempotent per connection: a repeat watch must NOT take a
                            // second manager refcount this teardown would never release.
                            let key = (repo.clone(), rel.clone());
                            if watched.contains(&key) {
                                continue;
                            }
                            let root = match registry::load_from(&registry_path) {
                                Ok(store) => store.entry(&repo).map(|e| PathBuf::from(&e.path)),
                                Err(e) => {
                                    tracing::warn!(error = %e, "tree watch: registry unreadable");
                                    None
                                }
                            };
                            let Some(root) = root else { continue };
                            if runs {
                                // `notify` errors on a missing path, and a repo where
                                // `ralphy run` never ran has no snapshot dir — without
                                // this, a first run started while the panel is open would
                                // stay invisible until reopen (ADR-0036 §4 amendment).
                                if let Err(e) = std::fs::create_dir_all(root.join(watch::RUNSTATE_REL))
                                {
                                    tracing::warn!(error = %e, "runs watch: creating the runstate dir");
                                }
                            }
                            match watchers.watch(&repo, &root, &rel) {
                                Ok(rx) => {
                                    // First dir for this repo on this connection → spawn its
                                    // forwarder on the rx the manager just handed us.
                                    forwarders
                                        .entry(repo.clone())
                                        .or_insert_with(|| spawn_nudge_forwarder(rx, nudge_tx.clone()));
                                    watched.push(key);
                                }
                                Err(e) => tracing::warn!(error = %e, "tree watch failed"),
                            }
                        }
                        "unwatch" | "runs.unwatch" => {
                            let rel = if cmd.verb == "runs.unwatch" {
                                watch::RUNSTATE_REL.to_string()
                            } else {
                                rel
                            };
                            if let Some(poller) = peer_pollers.get(&repo_ref) {
                                let key = (repo_ref.clone(), rel.clone());
                                if !watched.contains(&key) {
                                    continue;
                                }
                                poller
                                    .paths
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .remove(&rel);
                                watched.retain(|held| held != &key);
                                poller.runs.store(
                                    watched.iter().any(|(repo, path)| {
                                        repo == &repo_ref && path == watch::RUNSTATE_REL
                                    }),
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                                if !watched.iter().any(|(repo, _)| repo == &repo_ref) {
                                    if let Some(poller) = peer_pollers.remove(&repo_ref) {
                                        close_peer_tree_poller(poller);
                                    }
                                }
                                continue;
                            }
                            let repo = repo_ref.clone();
                            let key = (repo.clone(), rel.clone());
                            if !watched.contains(&key) {
                                continue; // not held → nothing to release (no double-unwatch)
                            }
                            watchers.unwatch(&repo, &rel);
                            watched.retain(|k| k != &key);
                            // Last dir of this repo released → stop its forwarder so a later
                            // re-watch re-subscribes to the rebuilt broadcast.
                            if !watched.iter().any(|(r, _)| r == &repo) {
                                if let Some(f) = forwarders.remove(&repo) {
                                    f.abort();
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
            nudge = nudge_rx.recv() => {
                // A repo's broadcast carries every watched dir; push only the ones
                // THIS connection subscribed to.
                if let Some((repo, rel)) = nudge {
                    if watched.iter().any(|(r, p)| r == &repo && p == &rel) {
                        // Discriminated by REL, not by the verb that subscribed, so
                        // both subscription kinds share ONE `watched` list (and one
                        // exactly-once teardown). The browser's two consumers react
                        // differently: re-read a subtree vs re-read `runs.list`.
                        if rel == watch::RUNSTATE_REL {
                            send_command(
                                &mut socket,
                                0,
                                "runs.dirty",
                                serde_json::json!({ "repo": repo }),
                            )
                            .await;
                        } else {
                            send_command(
                                &mut socket,
                                0,
                                "tree.dirty",
                                serde_json::json!({ "repo": repo, "path": rel }),
                            )
                            .await;
                        }
                    }
                }
            }
            exited = run_exits_rx.recv(), if exits_open => match exited {
                Ok(repo) => {
                    send_command(
                        &mut socket,
                        0,
                        "changes.dirty",
                        serde_json::json!({ "repo": repo }),
                    )
                    .await;
                }
                // Skipped nudges are free: the browser's re-read is idempotent.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                // Load-bearing: a closed broadcast makes `recv()` return
                // immediately forever, which would spin this loop.
                Err(tokio::sync::broadcast::error::RecvError::Closed) => exits_open = false,
            },
        }
    }

    // TEARDOWN: release every held dir (the last release tears the watcher down)
    // and abort the forwarders (their broadcast receivers may otherwise outlive us
    // if another connection keeps the repo alive).
    for (repo, rel) in &watched {
        if !peer_pollers.contains_key(repo) {
            watchers.unwatch(repo, rel);
        }
    }
    for (_repo, forwarder) in forwarders {
        forwarder.abort();
    }
    for (_repo, poller) in peer_pollers {
        close_peer_tree_poller(poller);
    }
}

/// Pipe one repo's `tree.dirty` broadcast into the connection's fan-in channel.
/// A lag just skips ahead (the browser re-reads idempotently); a closed broadcast
/// (the repo watcher torn down) or a dropped fan-in ends the task.
fn spawn_nudge_forwarder(
    mut rx: watch::DirtyRx,
    tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(item) => {
                    if tx.send(item).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// `GET /api/repos`: the registered repos as JSON, each with its live
/// reachability. Read FRESH from disk on every request so a separate `ralphy
/// run` process's write shows up on the next page refresh. A load error yields
/// an empty list with `200` (logged) rather than failing the page. `branch` is
/// likewise read fresh from `<path>/.git/HEAD`, `None` when it cannot be
/// determined (detached HEAD, unreachable repo, worktree gitdir pointer).
async fn repos_route(registry_path: PathBuf) -> Response {
    #[derive(serde::Serialize)]
    struct RepoView {
        slug: String,
        path: String,
        reachable: bool,
        branch: Option<String>,
        // Additive (#204): the real working-tree state and origin URL. Both spawn
        // `git`, so the whole `Vec` is built inside `spawn_blocking` below.
        dirty: bool,
        remote: Option<String>,
    }
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry; serving empty list");
            registry::RegistryStore::default()
        }
    };
    // `dirty`/`remote` each spawn a `git` subprocess per repo — that must not
    // block the async reactor, so the whole map runs on a blocking thread.
    let views = tokio::task::spawn_blocking(move || {
        store
            .repos
            .iter()
            .map(|(slug, entry)| RepoView {
                slug: slug.clone(),
                path: entry.path.clone(),
                reachable: entry.reachable(),
                branch: entry.head_branch(),
                dirty: entry.dirty(),
                remote: entry.remote(),
            })
            .collect::<Vec<RepoView>>()
    })
    .await
    .unwrap_or_default();
    Json(views).into_response()
}

/// `GET /api/usage[?since=<RFC3339 UTC, with `+` encoded as `%2B`>]`: the
/// token-usage ledger's run records PLUS the interactive records scanned from the
/// Claude and Codex stores, as `{ daemon_id, records: [...], interactive: [...] }` (ADR-0033
/// §2/§3). Both read FRESH from disk on every request, same as `/api/repos`.
/// `since` keeps run records whose `ts` is lexically `>=` it and interactive
/// records whose `last_ts` is `>=` it. The interactive scan excludes any session
/// the ledger already owns (its `session_id` in `records`) and writes nothing.
async fn usage_route(
    usage_dir: PathBuf,
    stores: StorePaths,
    registry_path: PathBuf,
    daemon_id: Option<String>,
    since: Option<String>,
) -> Response {
    let runs = usage::run_records(&usage_dir, since.as_deref());
    // A registry load error must not fail the page — serve interactive records
    // with no project/actor attribution, like `repos_route` (logged).
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry for the usage scan; serving unattributed");
            registry::RegistryStore::default()
        }
    };
    let interactive = usage::interactive_records(&stores, &store, &runs, since.as_deref());
    Json(serde_json::json!({
        "daemon_id": daemon_id,
        "records": runs,
        "interactive": interactive,
    }))
    .into_response()
}

/// `GET /api/desk`: the saved desk — windows and fences together, each in layout
/// order (ADR-0050, ADR-0051 §10). An absent or corrupt `desk.toml` answers
/// `200 {"windows":[],"fences":[]}` — a lost layout costs a cascaded stage,
/// never an error the shell has to handle.
async fn desk_get_route(path: PathBuf) -> Response {
    Json(desk::load_from(&path)).into_response()
}

/// `PUT /api/desk`: replace the desk wholesale, each record type pruned to its
/// own cap ([`desk::DESK_MAX`], [`desk::FENCE_MAX`]) newest by `ts`, answering
/// `200` with the pruned store — the client needs the daemon's post-prune truth
/// in one round trip (last-write-wins, no ETag).
///
/// A body that is not a `{ windows, fences }` object — including the pre-#340
/// bare array — is rejected by the `Json` extractor as `422` and never reaches
/// here, so `desk.toml` is untouched; a rect that is out of frame — non-finite,
/// or an origin off the stage's pinned 0,0 — is rejected here as `400`. Both
/// rejections return BEFORE any write, so a refused upload leaves `desk.toml`
/// byte-identical on every path.
///
/// Non-overlap between fences is deliberately NOT validated: refusing a whole
/// desk upload would cost the operator their layout and the daemon has no repair
/// path, so that invariant belongs to the client (ADR-0051 §6).
async fn desk_put_route(path: PathBuf, up: desk::DeskUpload) -> Response {
    if let Some(bad) = up.windows.iter().find(|r| !desk::rect_is_sane(&r.rect)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("record {} has an out-of-frame rect", bad.id) }),
            ),
        )
            .into_response();
    }
    if let Some(bad) = up.fences.iter().find(|f| !desk::rect_is_sane(&f.rect)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("fence {} has an out-of-frame rect", bad.id) }),
            ),
        )
            .into_response();
    }
    let store = desk::DeskStore {
        windows: desk::prune(up.windows),
        fences: desk::prune_fences(up.fences),
    };
    match desk::save_to(&store, &path) {
        Ok(()) => Json(store).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

/// `GET /api/sessions`: the daemon's live sessions as JSON, each with its
/// identity (`id`, `repo`, `agent`, `kind`, `started_at`) so the UI can list,
/// reattach, and close them. A WebSocket drop leaves its session here (the child
/// keeps running); only a close or the child exiting removes one.
async fn sessions_route(sessions: Arc<session::SessionManager>) -> Response {
    Json(sessions.list()).into_response()
}

/// `POST /api/sessions/close?id=<id>`: end a session (tree-kill its child, evict
/// any attached client). `200 {"closed":true}` when it existed, `404` otherwise.
async fn close_session_route(
    Query(q): Query<CloseQuery>,
    sessions: Arc<session::SessionManager>,
) -> Response {
    if sessions.close(q.id) {
        Json(serde_json::json!({ "closed": true })).into_response()
    } else {
        (StatusCode::NOT_FOUND, "unknown session").into_response()
    }
}

/// `GET /api/identity`: the loaded identity's `name`/`avatar` as JSON, or 404
/// when the daemon has not been baptized yet.
async fn identity_route(identity: Option<identity::Identity>) -> Response {
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

/// The last repo list each peer served, keyed by `daemon_id`. Held for the
/// router's lifetime — same ownership model as `sessions`/`watchers`, so the
/// public `router` signature holds.
type PeerRepoCache =
    Arc<std::sync::Mutex<std::collections::HashMap<String, registry::RegistryStore>>>;

/// One peer as the workbench sees it: who it is, where it runs, and what this
/// daemon just observed about it. `state` is the grouping key; `diagnosis` is
/// the sentence shown when that state is not `reachable`.
#[derive(serde::Serialize)]
struct PeerView {
    daemon_id: String,
    name: String,
    avatar: String,
    environment: String,
    state: String,
    diagnosis: String,
    /// Whether this peer advertised how to wake it (a WSL unit).
    nudgeable: bool,
}

/// Read the peer store off the reactor. A panic inside `read_store` degrades to
/// "no peers announced", so it is LOGGED rather than swallowed — silently
/// shorter is exactly the failure the fleet view must never show.
async fn read_peer_store(dir: PathBuf) -> (Vec<peer::PeerDescriptor>, Vec<peer::PeerReject>) {
    match tokio::task::spawn_blocking(move || peer::read_store(&dir)).await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!(error = %e, "reading the peer store failed; serving no peers");
            (Vec::new(), Vec::new())
        }
    }
}

/// `GET /api/fleet`: the federated repo view (ADR-0052 §5) — every peer this
/// daemon can see plus every repo the fleet knows, as
/// `{ peers: [...], repos: [...] }`.
///
/// Reads the peer store FRESH and probes every peer on EVERY request, holding no
/// background state: same contract as `/api/repos`. A cached liveness table would
/// contradict "a descriptor is a claim, not a fact" — the answer is what is true
/// now, not what was true when a poller last ran.
///
/// `/api/repos` is deliberately untouched: this route is additive, so nothing
/// pinned to the local list changes shape.
async fn fleet_route(
    registry_path: PathBuf,
    peers_dir: PathBuf,
    identity: Option<identity::Identity>,
    environment: String,
    repo_cache: PeerRepoCache,
) -> Response {
    let (descriptors, rejects) = read_peer_store(peers_dir.clone()).await;

    // Probe every peer CONCURRENTLY: with a 2 s per-peer timeout, dialling them
    // one after another would make the page cost the sum of the down ones.
    let mut set = tokio::task::JoinSet::new();
    for (index, d) in descriptors.iter().cloned().enumerate() {
        set.spawn(async move {
            let status = peer::client::probe(&d).await;
            // Only a reachable peer is asked for its repos; an unreachable one
            // contributes no rows but is still listed (marked, never removed).
            let store = match status {
                peer::client::PeerStatus::Reachable => {
                    match peer::client::get(&d, "/api/repos").await {
                        Ok((200, body)) => match fleet::store_from_repos_json(&body) {
                            Some(store) => Some(store),
                            None => {
                                tracing::warn!(peer = %d.environment, "peer served an /api/repos body this daemon cannot read; keeping its last-known repos");
                                None
                            }
                        },
                        Ok((code, _)) => {
                            tracing::warn!(peer = %d.environment, %code, "peer answered the handshake but refused /api/repos; keeping its last-known repos");
                            None
                        }
                        Err(e) => {
                            tracing::warn!(peer = %d.environment, error = %format!("{e:#}"), "could not read a peer's repos; keeping its last-known ones");
                            None
                        }
                    }
                }
                _ => None,
            };
            (index, status, store)
        });
    }
    let mut probed: Vec<Option<(peer::client::PeerStatus, Option<registry::RegistryStore>)>> =
        (0..descriptors.len()).map(|_| None).collect();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((index, status, store)) => probed[index] = Some((status, store)),
            Err(e) => tracing::warn!(error = %e, "a peer probe task failed"),
        }
    }

    // Remember what each peer just served, and recall it for the ones that could
    // not be asked. INVARIANT: the guard is taken and dropped inside this block —
    // it is a `std::sync::Mutex` and there is no `.await` between these lines.
    let recalled: Vec<Option<registry::RegistryStore>> = {
        let mut cache = match repo_cache.lock() {
            Ok(cache) => cache,
            Err(poisoned) => poisoned.into_inner(),
        };
        descriptors
            .iter()
            .zip(probed.iter())
            .map(
                |(d, slot)| match slot.as_ref().and_then(|(_, s)| s.as_ref()) {
                    Some(fresh) => {
                        cache.insert(d.daemon_id.clone(), fresh.clone());
                        Some(fresh.clone())
                    }
                    None => cache.get(&d.daemon_id).cloned(),
                },
            )
            .collect()
    };

    let mut peer_views: Vec<PeerView> = Vec::with_capacity(descriptors.len() + rejects.len());
    let mut aggregate_input: Vec<(
        &peer::PeerDescriptor,
        &peer::client::PeerStatus,
        Option<&registry::RegistryStore>,
    )> = Vec::with_capacity(descriptors.len());
    // A probe whose task itself failed is reported as unreachable rather than
    // dropped — the operator must see the peer, not a silently shorter list.
    let fallback = peer::client::PeerStatus::Unreachable {
        why: "the probe did not complete".to_string(),
    };
    for ((d, slot), store) in descriptors.iter().zip(probed.iter()).zip(recalled.iter()) {
        let status = match slot {
            Some((status, _)) => status,
            None => &fallback,
        };
        let store = store.as_ref();
        peer_views.push(PeerView {
            daemon_id: d.daemon_id.clone(),
            name: d.name.clone(),
            avatar: d.avatar.clone(),
            environment: d.environment.clone(),
            state: status.state().to_string(),
            diagnosis: status.diagnosis(&d.environment),
            nudgeable: d.nudge.is_some(),
        });
        aggregate_input.push((d, status, store));
    }
    // A rejected record is DEGRADED, never dropped: the operator has a file on
    // disk that is not doing what they think it is, and only this list says so.
    for reject in &rejects {
        peer_views.push(PeerView {
            // A synthetic id keyed on the FILE: the browser groups by
            // `daemon_id`, so a shared empty string would collapse two bad
            // descriptors into one row and lose the first one's diagnosis.
            daemon_id: format!("malformed:{}", reject.file()),
            name: reject.file().to_string(),
            avatar: "❔".to_string(),
            environment: "unknown".to_string(),
            state: "malformed".to_string(),
            diagnosis: reject.why(),
            nudgeable: false,
        });
    }

    let local_store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry; federating an empty local list");
            registry::RegistryStore::default()
        }
    };
    let local_id = identity
        .as_ref()
        .map(|i| i.id.to_string())
        .unwrap_or_default();
    let local_name = identity
        .as_ref()
        .map(|i| i.name.clone())
        .unwrap_or_default();
    let repos = fleet::aggregate(
        (&local_id, &local_name, &environment, &local_store),
        &aggregate_input,
    );
    Json(serde_json::json!({ "peers": peer_views, "repos": repos })).into_response()
}

/// The `daemon_id` a nudge is aimed at.
#[derive(serde::Deserialize)]
struct NudgeQuery {
    daemon_id: String,
}

/// `POST /api/fleet/nudge?daemon_id=<id>`: ask the OS to start a peer that is not
/// answering (ADR-0052 §4). Fire-and-forget — the daemon spawns and never
/// parents, holds, or signals what it started.
async fn fleet_nudge_route(peers_dir: PathBuf, daemon_id: String) -> Response {
    let (descriptors, _rejects) = read_peer_store(peers_dir.clone()).await;
    let Some(d) = descriptors.into_iter().find(|d| d.daemon_id == daemon_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("no peer announced as {daemon_id}") })),
        )
            .into_response();
    };
    let Some(spec) = d.nudge.as_ref() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "peer {} announced no way to wake it — a WSL peer needs `loginctl enable-linger` \
                     and a `ralphy-daemon.service` user unit before it can be nudged",
                    d.environment
                )
            })),
        )
            .into_response();
    };
    let argv = peer::nudge::nudge_argv(spec);
    match peer::nudge::spawn_detached(&argv) {
        Ok(()) => Json(serde_json::json!({ "nudged": true })).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

/// `GET /api/peer/hello`: the local fleet's version handshake (ADR-0052 §3) —
/// who this daemon is, which environment it runs in, and which peer protocol it
/// speaks. Cheap and side-effect-free, and deliberately NOT folded into
/// `/api/identity`, whose 404-when-un-baptized contract the browser depends on.
///
/// 404 when un-baptized, matching `/api/identity`: a daemon with no identity has
/// nothing a peer could key on.
async fn peer_hello_route(identity: Option<identity::Identity>, environment: String) -> Response {
    #[derive(serde::Serialize)]
    struct HelloView {
        daemon_id: String,
        name: String,
        avatar: String,
        environment: String,
        protocol_version: u32,
    }
    match identity {
        Some(id) => Json(HelloView {
            daemon_id: id.id.to_string(),
            name: id.name,
            avatar: id.avatar,
            environment,
            protocol_version: peer::PEER_PROTOCOL_VERSION,
        })
        .into_response(),
        None => (StatusCode::NOT_FOUND, "no identity").into_response(),
    }
}

/// Execute a peer command against this daemon's local registry only.
///
/// The repo is a bare slug here. This boundary never routes again, which makes
/// proxy loops unrepresentable.
async fn peer_command_route(
    registry_path: PathBuf,
    daemon_id: Option<String>,
    Json(cmd): Json<protocol::Command>,
) -> Response {
    let Some(verb) = dispatch::Verb::from_query(&cmd.verb) else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "unknown verb"
        }))
        .into_response();
    };
    if verb.effect_class() == dispatch::EffectClass::Spawn {
        return Json(serde_json::json!({
            "status": "error",
            "message": "a run is not federated yet"
        }))
        .into_response();
    }
    let slug = cmd
        .payload
        .get("repo")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry for a peer command");
            return Json(serde_json::json!({
                "status": "error",
                "message": "repo registry unreadable"
            }))
            .into_response();
        }
    };
    let Some(entry) = store.entry(slug) else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "unknown repo"
        }))
        .into_response();
    };
    let payload = execute_oneshot(verb, &cmd, Path::new(&entry.path), daemon_id.as_deref())
        .await
        .unwrap_or_else(|| {
            serde_json::json!({
                "status": "error",
                "message": "a run is not federated yet"
            })
        });
    Json(payload).into_response()
}

#[derive(serde::Deserialize)]
struct PeerTreePoll {
    sub: String,
    repo: String,
    paths: Vec<String>,
    runs: bool,
    timeout_ms: u64,
}

#[derive(serde::Deserialize)]
struct PeerTreeClose {
    sub: String,
}

/// Long-poll one buffered tree subscription against this daemon's local repo.
async fn peer_tree_poll_route(
    registry_path: PathBuf,
    subs: Arc<fleet::watchsub::WatchSubs>,
    Json(mut poll): Json<PeerTreePoll>,
) -> Response {
    subs.sweep(fleet::watchsub::IDLE_EXPIRY);
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(_) => {
            return Json(serde_json::json!({
                "status": "error",
                "message": "repo registry unreadable"
            }))
            .into_response()
        }
    };
    let Some(entry) = store.entry(&poll.repo) else {
        return Json(serde_json::json!({
            "status": "error",
            "message": "unknown repo"
        }))
        .into_response();
    };
    let root = Path::new(&entry.path);
    if poll.runs {
        let runstate = root.join(watch::RUNSTATE_REL);
        if let Err(e) = std::fs::create_dir_all(&runstate) {
            tracing::warn!(path = %runstate.display(), error = %e, "failed to create peer runstate watch directory");
        }
        if !poll.paths.iter().any(|path| path == watch::RUNSTATE_REL) {
            poll.paths.push(watch::RUNSTATE_REL.to_string());
        }
    }
    if let Err(e) = subs.subscribe(&poll.sub, &poll.repo, root, &poll.paths) {
        tracing::warn!(error = %e, "refused a peer tree subscription");
        return Json(serde_json::json!({ "dirty": [] })).into_response();
    }
    let dirty = subs
        .wait(
            &poll.sub,
            Duration::from_millis(poll.timeout_ms.min(25_000)),
        )
        .await;
    let dirty: Vec<serde_json::Value> = dirty
        .into_iter()
        .map(|(repo, path)| serde_json::json!({ "repo": repo, "path": path }))
        .collect();
    Json(serde_json::json!({ "dirty": dirty })).into_response()
}

async fn peer_tree_close_route(
    subs: Arc<fleet::watchsub::WatchSubs>,
    Json(close): Json<PeerTreeClose>,
) -> Response {
    subs.close(&close.sub);
    Json(serde_json::json!({ "closed": true })).into_response()
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
async fn about_route() -> Response {
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

/// `GET /api/agents`: the daemon's own adapter enumeration (`id`, `label`,
/// `accelerator`), so the workbench console menu renders from the daemon rather
/// than from a second vendor list of its own. Read-only, no secrets; it reports
/// what the daemon can launch, never whether a vendor CLI is installed here.
async fn agents_route() -> Response {
    Json(roster::roster()).into_response()
}

/// The `POST /api/login` form: the current TOTP `code` and, when a password is
/// enrolled, the operator's `password`. `password` is `Option` so a bind with no
/// password enrolled accepts a form carrying only `code`.
#[derive(serde::Deserialize)]
struct LoginForm {
    code: String,
    password: Option<String>,
}

/// `POST /api/login`: validate the TOTP code (and password, if enrolled) against
/// the CURRENT `Session` policy. On success `200` + a `Set-Cookie: ralphy_session=…`
/// header; on a bad credential `401`; while rate-limited `429` with a
/// `Retry-After` (amendment §D). Login is meaningless without a `Session` policy,
/// so any other policy returns `404`.
async fn login_submit(state: Arc<auth::AuthState>, Form(form): Form<LoginForm>) -> Response {
    // Throttle first: a 6-digit TOTP is otherwise online-brute-forceable.
    if let Err(retry_after) = state.throttle_check() {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after.to_string())],
            "too many attempts — try again shortly",
        )
            .into_response();
    }
    let auth::AuthPolicy::Session(session) = state.policy() else {
        return (StatusCode::NOT_FOUND, "login not enabled").into_response();
    };
    let now = now_unix();
    // The last consumed TOTP step gates anti-replay (amendment §D); the store lives
    // on the AuthState (real store at boot, detached temp in tests).
    let last_step = state.last_step();
    match session.login_checked(&form.code, form.password.as_deref(), now, last_step) {
        auth::LoginOutcome::Ok { cookie, step } => {
            // Persist the consumed step so the same code can't be replayed.
            state.record_step(step);
            state.throttle_record(true);
            (
                StatusCode::OK,
                [(header::SET_COOKIE, cookie::set_cookie_value(&cookie))],
            )
                .into_response()
        }
        // A replay and a bad credential are indistinguishable to the client (generic
        // message) and both feed the throttle.
        auth::LoginOutcome::BadCredential | auth::LoginOutcome::Replayed => {
            state.throttle_record(false);
            (StatusCode::UNAUTHORIZED, "invalid credentials").into_response()
        }
    }
}

/// Serve a file from the embedded UI tree; `/` means `index.html`.
async fn ui_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match UI.get_file(path) {
        Some(file) => (
            [(header::CONTENT_TYPE, content_type(path))],
            file.contents(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// `POST /api/logout`: bump the session epoch so the cookie is invalidated
/// SERVER-SIDE (amendment §B — not merely cleared client-side), then emit a
/// `Max-Age=0` clearing `Set-Cookie`. The cookie is `HttpOnly`, so JS cannot
/// clear it — the server must (issue #186).
async fn logout_route(state: Arc<auth::AuthState>) -> Response {
    if let Err(e) = state.invalidate_sessions() {
        // A failed epoch bump must not strand the operator "logged in": clearing
        // the cookie still drops this browser. Log and proceed.
        tracing::warn!(error = %e, "failed to bump the session epoch on logout");
    }
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie::clear_cookie_value())],
    )
        .into_response()
}

/// The SPA's auth-state oracle, reachable pre-login (allowlisted). Drives the
/// workbench gate's `authed` flag, password-field visibility, and the Security
/// modal's policy-aware affordances (issue #205).
#[derive(serde::Serialize)]
struct SessionState {
    authed: bool,
    password: bool,
    policy: &'static str,
    /// The daemon's avatar, so the login card wears THIS daemon's face instead of
    /// a generic robot — an operator with several daemons open has nothing else
    /// on that screen to tell them apart by.
    ///
    /// The NAME is deliberately not here. This route is pre-login: everything on
    /// it is readable without a cookie, and the login gate is meant to be opaque
    /// (see [`require_auth`]). An avatar is one emoji drawn from the fixed,
    /// source-visible pool in [`identity::AVATARS`] — it identifies nothing an
    /// attacker did not already have (they are looking at the daemon), while a
    /// name is the operator's own words about their machine.
    #[serde(skip_serializing_if = "Option::is_none")]
    avatar: Option<String>,
}

/// `GET /api/session`: report whether this request is authorized and whether a
/// password factor is enrolled. `Localhost`/`Bearer` are always `authed`; under a
/// `Session` policy `authed` reflects a valid `Bearer` OR session cookie.
async fn session_state_route(
    state: Arc<auth::AuthState>,
    avatar: Option<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    let auth = state.policy();
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let authed = auth.authorizes(bearer)
        || match &auth {
            auth::AuthPolicy::Session(s) => {
                let cookie_header = headers.get(header::COOKIE).and_then(|v| v.to_str().ok());
                s.cookie_valid(cookie_header, now_unix())
            }
            _ => false,
        };
    let password = matches!(&auth, auth::AuthPolicy::Session(s) if s.password.is_some());
    Json(SessionState {
        authed,
        password,
        policy: auth.name(),
        avatar,
    })
    .into_response()
}

/// The daemon's auth-state surface for the Security modal (issue #195): which
/// factors are enrolled in the REAL stores. `require_login` is the PERSISTED
/// `daemon-require-login` flag (ADR-0032 amendment §A): an operator opt-in that
/// gates the browser UI even on a loopback bind, no longer derived from the seed.
#[derive(serde::Serialize)]
struct SecurityState {
    token_set: bool,
    password_set: bool,
    totp_enrolled: bool,
    require_login: bool,
}

/// Read the real store FILES under `dir` and report enrolment. Path-explicit (no
/// env reads) so tests pass a tempdir. `require_login` is now the PERSISTED flag
/// (ADR-0032 amendment §A), no longer derived from the seed.
fn security_state_at(dir: &Path) -> SecurityState {
    let totp_enrolled = totp::load_seed_from(&totp::seed_path_in(dir))
        .ok()
        .flatten()
        .is_some();
    SecurityState {
        token_set: auth::load_token_from(&auth::token_path_in(dir))
            .ok()
            .flatten()
            .is_some(),
        password_set: password::load_from(&password::password_path_in(dir))
            .ok()
            .flatten()
            .is_some(),
        totp_enrolled,
        require_login: auth::require_login_enabled_in(dir),
    }
}

/// Begin enrolment: mint-once a PENDING TOTP seed under `dir` and return its
/// `otpauth://` URI + whether it was newly minted. The seed is NOT armed — it
/// gates nothing until [`confirm_totp_at`] verifies a code (ADR-0032 amendment
/// §C). The URI is shown once (QR + base32); a re-enrol before confirming
/// returns the SAME pending secret with `newly_minted=false`.
fn enroll_totp_at(dir: &Path) -> Result<(String, bool)> {
    let (seed, newly_minted) = totp::ensure_seed_at(&totp::pending_seed_path_in(dir))?;
    Ok((seed.otpauth_uri("ralphy", "daemon"), newly_minted))
}

/// Confirm a pending enrolment: verify `code` against the pending seed and, on
/// success, promote it to the live seed. Returns whether the code verified.
fn confirm_totp_at(dir: &Path, code: &str, now: u64) -> Result<bool> {
    totp::confirm_pending_at(
        &totp::pending_seed_path_in(dir),
        &totp::seed_path_in(dir),
        code,
        now,
    )
}

/// Revoke enrolment: delete the live seed, any in-flight pending seed, and the
/// anti-replay last-step marker, so an abandoned enrolment leaves nothing behind
/// (ADR-0032 amendment §C/§D).
fn revoke_totp_at(dir: &Path) -> Result<()> {
    totp::revoke_seed_at(&totp::seed_path_in(dir))?;
    totp::revoke_seed_at(&totp::pending_seed_path_in(dir))?;
    totp::revoke_seed_at(&totp::last_step_path_in(dir))
}

/// Set (non-empty) or clear (empty/absent) the optional password under `dir`;
/// return whether a password is now enrolled.
fn set_password_at(dir: &Path, password: Option<&str>) -> Result<bool> {
    let path = password::password_path_in(dir);
    match password.filter(|p| !p.is_empty()) {
        Some(pw) => {
            password::save_to(&password::Hash::hash_password(pw), &path)?;
            Ok(true)
        }
        None => {
            password::clear_at(&path)?;
            Ok(false)
        }
    }
}

/// Remint the access token under `dir`, overwriting any prior. The token is
/// never echoed — only its rotation is reported.
fn remint_token_at(dir: &Path) -> Result<()> {
    auth::save_token_to(&auth::generate_token(), &auth::token_path_in(dir))
}

/// The require-login gate (ADR-0032 amendment §A): persist the operator's choice
/// as the `daemon-require-login` flag. Enabling demands an armed TOTP seed
/// (`Err("totp not enrolled")` otherwise) and MINTS the access token if absent —
/// gating a loopback bind needs a signing key, and machine clients then use it as
/// a bearer. Disabling just clears the flag.
fn require_login_at(dir: &Path, enable: bool) -> Result<()> {
    if enable {
        if totp::load_seed_from(&totp::seed_path_in(dir))
            .ok()
            .flatten()
            .is_none()
        {
            anyhow::bail!("totp not enrolled");
        }
        // Ensure a signing key exists (mint-once) so the gate can sign cookies —
        // a loopback bind may never have minted one.
        auth::ensure_token_at(&auth::token_path_in(dir))?;
    }
    auth::set_require_login_in(dir, enable)
}

/// `GET /api/security/state`: the real enrolment state (gated by `require_auth`;
/// not in the login allowlist).
async fn security_state_route() -> Response {
    match auth::store_dir() {
        Ok(dir) => Json(security_state_at(&dir)).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "failed to resolve the daemon store for security state");
            (StatusCode::INTERNAL_SERVER_ERROR, "store unavailable").into_response()
        }
    }
}

/// `POST /api/security/totp/enroll`: mint-once the seed and return the one-time
/// `otpauth://` URI + `newly_minted`.
async fn security_totp_enroll_route() -> Response {
    match auth::store_dir().and_then(|dir| enroll_totp_at(&dir)) {
        Ok((uri, newly_minted)) => {
            Json(serde_json::json!({ "uri": uri, "newly_minted": newly_minted })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to enroll a TOTP seed");
            (StatusCode::INTERNAL_SERVER_ERROR, "enroll failed").into_response()
        }
    }
}

/// The `POST /api/security/totp/confirm` body: the live 6-digit code proving the
/// operator scanned the pending QR.
#[derive(serde::Deserialize)]
struct ConfirmForm {
    code: String,
}

/// `POST /api/security/totp/confirm`: verify `code` against the pending seed and
/// arm it on success. `200 {confirmed}` either way; a wrong code is
/// `confirmed:false` (not an error) so the UI can prompt a retry. On a successful
/// arm the live policy is rebuilt (a promotion takes effect if require-login is
/// already on).
async fn security_totp_confirm_route(
    state: Arc<auth::AuthState>,
    Form(form): Form<ConfirmForm>,
) -> Response {
    let now = now_unix();
    match auth::store_dir().and_then(|dir| confirm_totp_at(&dir, &form.code, now)) {
        Ok(confirmed) => {
            if confirmed {
                apply_auth_change(&state, false);
            }
            Json(serde_json::json!({ "confirmed": confirmed })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to confirm the TOTP enrolment");
            (StatusCode::INTERNAL_SERVER_ERROR, "confirm failed").into_response()
        }
    }
}

/// `POST /api/security/totp/revoke`: delete the live AND pending seeds (mint-once
/// posture). Rebuilds the policy (demoting a gated bind) and invalidates live
/// sessions — revoking the factor must drop anyone it authorized.
async fn security_totp_revoke_route(state: Arc<auth::AuthState>) -> Response {
    match auth::store_dir().and_then(|dir| revoke_totp_at(&dir)) {
        Ok(()) => {
            apply_auth_change(&state, true);
            Json(serde_json::json!({ "revoked": true })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to revoke the TOTP seed");
            (StatusCode::INTERNAL_SERVER_ERROR, "revoke failed").into_response()
        }
    }
}

/// The `POST /api/security/password` body: a non-empty `password` sets it, an
/// empty/absent one clears it.
#[derive(serde::Deserialize)]
struct PasswordForm {
    password: Option<String>,
}

/// `POST /api/security/password`: set or clear the optional password factor.
/// Rebuilds the policy (the `Session` carries the new/absent password) and
/// invalidates live sessions so they re-authenticate under the changed factor.
async fn security_password_route(
    state: Arc<auth::AuthState>,
    Form(form): Form<PasswordForm>,
) -> Response {
    match auth::store_dir().and_then(|dir| set_password_at(&dir, form.password.as_deref())) {
        Ok(password_set) => {
            apply_auth_change(&state, true);
            Json(serde_json::json!({ "password_set": password_set })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to update the password");
            (StatusCode::INTERNAL_SERVER_ERROR, "password update failed").into_response()
        }
    }
}

/// `POST /api/security/token/remint`: rotate the access token (never echoed).
/// The token is the cookie signing key, so rebuild the policy under the new key
/// and invalidate live sessions — a re-mint logs everyone out (amendment §B),
/// now IMMEDIATELY rather than at next restart.
async fn security_token_remint_route(state: Arc<auth::AuthState>) -> Response {
    match auth::store_dir().and_then(|dir| remint_token_at(&dir)) {
        Ok(()) => {
            apply_auth_change(&state, true);
            Json(serde_json::json!({ "reminted": true })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to remint the access token");
            (StatusCode::INTERNAL_SERVER_ERROR, "remint failed").into_response()
        }
    }
}

/// The `POST /api/security/require-login` body: the desired toggle state.
#[derive(serde::Deserialize)]
struct RequireLoginForm {
    enable: bool,
}

/// `POST /api/security/require-login`: persist the operator's gate choice
/// (amendment §A). Enabling without an armed TOTP seed is refused (`400`, AC4);
/// enabling mints a signing token if absent. Either way the policy is rebuilt so
/// the gate engages/lifts IMMEDIATELY, and live sessions are invalidated (turning
/// the gate on logs the browser off; turning it off re-issues cleanly).
async fn security_require_login_route(
    state: Arc<auth::AuthState>,
    Form(form): Form<RequireLoginForm>,
) -> Response {
    match auth::store_dir().and_then(|dir| require_login_at(&dir, form.enable)) {
        Ok(()) => {
            apply_auth_change(&state, true);
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// Apply a security mutation to the LIVE auth state: rebuild the policy from disk
/// so the change takes effect without a restart (amendment §A), and — when
/// `invalidate` — bump the session epoch to drop outstanding cookies (§B).
/// Failures are logged, never fatal to the request that triggered them (the store
/// write already succeeded; a stale in-memory policy self-heals on restart).
fn apply_auth_change(state: &auth::AuthState, invalidate: bool) {
    if let Err(e) = state.rebuild() {
        tracing::warn!(error = %e, "failed to rebuild the live auth policy");
    }
    if invalidate {
        if let Err(e) = state.invalidate_sessions() {
            tracing::warn!(error = %e, "failed to invalidate sessions");
        }
    }
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("eot") => "application/vnd.ms-fontobject",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

/// Resolves when the operator asks the foreground daemon to stop. Ctrl+C maps
/// to a console event on Windows and SIGINT on Unix — `tokio::signal::ctrl_c`
/// covers both, keeping shutdown cross-platform without cfg splits.
async fn shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %e, "failed to listen for Ctrl+C; running until killed");
        std::future::pending::<()>().await;
    }
    tracing::info!("shutdown requested (Ctrl+C)");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// A never-fired shutdown receiver for the in-process router tests (none of
    /// them exercise `/ws`, so its sender dropping immediately is harmless).
    fn idle_shutdown() -> tokio::sync::watch::Receiver<bool> {
        tokio::sync::watch::channel(false).1
    }

    async fn get(path: &str) -> Response {
        router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
    }

    /// A router rooted at a SCRATCH registry path, so its `desk.toml` sibling
    /// lands in a temp dir. The shared `get()` helper passes a relative
    /// `does-not-exist`, whose desk sibling would be written into the process
    /// cwd — every test that PUTs must build its router this way.
    fn desk_router(dir: &Path) -> Router {
        router(
            None,
            dir.join("repos.toml"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
    }

    async fn body_text(res: Response) -> String {
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
    }

    async fn desk_get(dir: &Path) -> String {
        let res = desk_router(dir)
            .oneshot(
                Request::builder()
                    .uri("/api/desk")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        body_text(res).await
    }

    /// PUT a RAW body — the only way to exercise a shape the `DeskUpload`
    /// extractor must refuse (a bare array, an out-of-range float literal).
    async fn desk_put_raw(dir: &Path, body: String) -> Response {
        desk_router(dir)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/desk")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn desk_put(dir: &Path, body: &serde_json::Value) -> Response {
        desk_put_raw(dir, body.to_string()).await
    }

    /// The `{ windows, fences }` upload body (#340).
    fn desk_body(windows: serde_json::Value, fences: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "windows": windows, "fences": fences })
    }

    fn fence_json(id: &str, name: &str, ts: i64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": name,
            "rect": { "left": 40.0, "top": 40.0, "width": 720.0, "height": 460.0 },
            "ts": ts,
        })
    }

    fn desk_json(id: &str, ts: i64, session_id: serde_json::Value, max: bool) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "repo": "owner/repo",
            "agent": "claude",
            "kind": "console",
            "rect": { "left": 10.0, "top": 20.0, "width": 640.0, "height": 480.0 },
            "max": max,
            "sessionId": session_id,
            "ts": ts,
        })
    }

    #[tokio::test]
    async fn api_desk_empty_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(desk_get(dir.path()).await, r#"{"windows":[],"fences":[]}"#);
        assert!(
            !dir.path().join("desk.toml").exists(),
            "a GET must not create the store"
        );
    }

    /// The desk route's body is an OBJECT carrying both record types (#340), so
    /// an empty desk is `{"windows":[],"fences":[]}` — not a bare `[]`.
    #[tokio::test]
    async fn api_desk_serves_windows_and_fences_together() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            desk_get(dir.path()).await,
            r#"{"windows":[],"fences":[]}"#,
            "the desk body carries both record types"
        );
    }

    #[tokio::test]
    async fn api_desk_put_then_get_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let payload = desk_body(
            serde_json::json!([
                desk_json("w-a", 1, serde_json::json!(7), true),
                desk_json("w-b", 2, serde_json::Value::Null, false),
            ]),
            serde_json::json!([]),
        );
        let res = desk_put(dir.path(), &payload).await;
        assert_eq!(res.status(), StatusCode::OK);

        let body = desk_get(dir.path()).await;
        assert!(
            body.contains("\"sessionId\":7"),
            "camelCase wire key: {body}"
        );
        assert!(body.contains("\"max\":true"), "maximized survives: {body}");
        let a = body.find("w-a").expect("first record present");
        let b = body.find("w-b").expect("second record present");
        assert!(a < b, "layout order is preserved: {body}");
    }

    #[tokio::test]
    async fn api_desk_put_prunes_to_24_newest_by_ts() {
        let dir = tempfile::tempdir().unwrap();
        let payload = desk_body(
            serde_json::Value::Array(
                (1..=30)
                    .map(|n| desk_json(&format!("w{n}"), n, serde_json::Value::Null, false))
                    .collect(),
            ),
            serde_json::json!([]),
        );
        let res = desk_put(dir.path(), &payload).await;
        assert_eq!(res.status(), StatusCode::OK);
        let put_body: desk::DeskStore = serde_json::from_str(&body_text(res).await).unwrap();
        let ids: Vec<String> = put_body.windows.into_iter().map(|r| r.id).collect();
        let expected: Vec<String> = (7..=30).map(|n| format!("w{n}")).collect();
        assert_eq!(ids, expected, "the PUT answers with the pruned truth");

        let get_body: desk::DeskStore = serde_json::from_str(&desk_get(dir.path()).await).unwrap();
        let ids: Vec<String> = get_body.windows.into_iter().map(|r| r.id).collect();
        assert_eq!(ids, expected, "and the persisted desk holds the same 24");
    }

    #[tokio::test]
    async fn api_desk_put_rejects_a_malformed_body_without_touching_the_store() {
        let dir = tempfile::tempdir().unwrap();
        desk_put(
            dir.path(),
            &desk_body(
                serde_json::json!([desk_json("w-a", 1, serde_json::Value::Null, false)]),
                serde_json::json!([]),
            ),
        )
        .await;
        let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

        let res = desk_put(dir.path(), &serde_json::json!({ "not": "an array" })).await;
        assert_eq!(
            res.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "the strict DeskUpload extractor rejects an unknown-field body — it \
             must never read as an EMPTY desk that wipes the layout"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "a rejected upload never reaches the store"
        );
    }

    #[tokio::test]
    async fn api_desk_put_rejects_a_negative_left_without_touching_the_store() {
        let dir = tempfile::tempdir().unwrap();
        desk_put(
            dir.path(),
            &desk_body(
                serde_json::json!([desk_json("w-a", 1, serde_json::Value::Null, false)]),
                serde_json::json!([]),
            ),
        )
        .await;
        let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

        let mut bad = desk_json("w-neg", 2, serde_json::Value::Null, false);
        bad["rect"]["left"] = serde_json::json!(-1.0);
        let res = desk_put(
            dir.path(),
            &desk_body(serde_json::json!([bad]), serde_json::json!([])),
        )
        .await;
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "the stage origin is pinned at 0,0 — a negative left is off the plane"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "a rejected rect never reaches the store"
        );
    }

    #[tokio::test]
    async fn api_desk_put_rejects_a_non_finite_rect_without_touching_the_store() {
        let dir = tempfile::tempdir().unwrap();
        desk_put(
            dir.path(),
            &desk_body(
                serde_json::json!([desk_json("w-a", 1, serde_json::Value::Null, false)]),
                serde_json::json!([]),
            ),
        )
        .await;
        let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

        // An out-of-range literal, spelled in the RAW body — a Rust `1e400_f64`
        // will not compile, and `json!(f64::INFINITY)` becomes `null`, so the
        // only way to reproduce what a browser can actually send is the wire.
        let bad = desk_json("w-huge", 2, serde_json::Value::Null, false)
            .to_string()
            .replace("\"left\":10.0", "\"left\":1e400");
        let res = desk_put_raw(dir.path(), format!(r#"{{"windows":[{bad}],"fences":[]}}"#)).await;
        assert_ne!(
            res.status(),
            StatusCode::OK,
            "a non-finite rect must never be stored"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "a rejected rect never reaches the store"
        );
    }

    /// The pre-#340 wire shape must be refused WHOLESALE, not half-applied: a
    /// stale client that still PUTs a bare array would otherwise be read as an
    /// empty desk and wipe the operator's layout.
    #[tokio::test]
    async fn api_desk_put_rejects_the_pre_340_bare_array() {
        let dir = tempfile::tempdir().unwrap();
        desk_put(
            dir.path(),
            &desk_body(
                serde_json::json!([desk_json("w-a", 1, serde_json::Value::Null, false)]),
                serde_json::json!([fence_json("f-a", "backend", 1)]),
            ),
        )
        .await;
        let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

        let stale = serde_json::json!([desk_json("w-b", 2, serde_json::Value::Null, false)]);
        let res = desk_put_raw(dir.path(), stale.to_string()).await;
        assert_eq!(
            res.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "the bare-array body is not a desk upload any more"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "a rejected upload never reaches the store"
        );

        // Every sequence shape that could satisfy the struct POSITIONALLY, each
        // its own leg — these are the bodies that wipe the desk when they land,
        // and each defeats a different half-fix. `[]` needs both fields
        // defaulted; `[[],[]]` supplies both required fields as two elements and
        // survived dropping the defaults; a map missing one key is the shape
        // that goes green again if `#[serde(default)]` is ever restored to a
        // single field. All three were measured green-then-red on this route.
        for body in ["[]", "[[],[]]", r#"{"windows":[]}"#, r#"{"fences":[]}"#] {
            let res = desk_put_raw(dir.path(), body.into()).await;
            assert_eq!(
                res.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "`{body}` must not read as a desk upload"
            );
            assert_eq!(
                std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
                before,
                "the operator's desk survives `{body}`"
            );
        }
    }

    #[tokio::test]
    async fn api_desk_put_rejects_a_fence_with_a_non_finite_rect() {
        let dir = tempfile::tempdir().unwrap();
        desk_put(
            dir.path(),
            &desk_body(
                serde_json::json!([]),
                serde_json::json!([fence_json("f-a", "backend", 1)]),
            ),
        )
        .await;
        let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

        // LEG 1 — the wire. Measured: `serde_json` refuses an out-of-range float
        // literal as a SYNTAX error ("number out of range"), which axum maps to
        // 400 — so a non-finite rect dies in the extractor and never reaches the
        // route's own guard. Same status, different body.
        let bad = fence_json("w-huge", "planning", 2)
            .to_string()
            .replace("\"left\":40.0", "\"left\":1e999");
        let res = desk_put_raw(dir.path(), format!(r#"{{"windows":[],"fences":[{bad}]}}"#)).await;
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "an out-of-range literal dies in the extractor"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "a rejected fence never reaches the store"
        );

        // LEG 2 — the route's OWN guard, which the wire can no longer reach:
        // called directly with an infinity the extractor would have refused, so
        // the 400 and its wording are proved rather than assumed. A fresh
        // response, not the one leg 1 asserted on.
        let res = desk_put_route(
            dir.path().join("desk.toml"),
            desk::DeskUpload {
                windows: vec![],
                fences: vec![desk::DeskFence {
                    id: "w-huge".into(),
                    name: "planning".into(),
                    rect: desk::DeskRect {
                        left: f64::INFINITY,
                        top: 40.0,
                        width: 720.0,
                        height: 460.0,
                    },
                    ts: 2,
                }],
            },
        )
        .await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let body = body_text(res).await;
        assert!(
            body.contains("fence w-huge has an out-of-frame rect"),
            "the refusal names the fence: {body}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "the guard returns BEFORE any write"
        );
    }

    #[tokio::test]
    async fn api_desk_put_prunes_fences_to_the_12_newest() {
        let dir = tempfile::tempdir().unwrap();
        let payload = desk_body(
            serde_json::json!([]),
            serde_json::Value::Array(
                (1..=13)
                    .map(|n| fence_json(&format!("f{n}"), "region", n))
                    .collect(),
            ),
        );
        let res = desk_put(dir.path(), &payload).await;
        assert_eq!(res.status(), StatusCode::OK);
        let put_body: desk::DeskStore = serde_json::from_str(&body_text(res).await).unwrap();
        let ids: Vec<String> = put_body.fences.into_iter().map(|f| f.id).collect();
        let expected: Vec<String> = (2..=13).map(|n| format!("f{n}")).collect();
        assert_eq!(ids, expected, "the PUT answers with the pruned truth");

        let get_body: desk::DeskStore = serde_json::from_str(&desk_get(dir.path()).await).unwrap();
        let ids: Vec<String> = get_body.fences.into_iter().map(|f| f.id).collect();
        assert_eq!(ids, expected, "and the persisted desk holds the same 12");
    }

    #[test]
    fn security_state_reflects_the_stores() {
        let dir = tempfile::tempdir().unwrap();
        // Empty store → every factor unset.
        let s = security_state_at(dir.path());
        assert!(!s.token_set && !s.password_set && !s.totp_enrolled && !s.require_login);
        // Writing a seed flips totp_enrolled — but require_login is now the
        // PERSISTED opt-in flag (amendment §A), NOT derived from the seed.
        totp::save_seed_to(&totp::generate_seed(), &totp::seed_path_in(dir.path())).unwrap();
        let s = security_state_at(dir.path());
        assert!(
            s.totp_enrolled && !s.require_login,
            "seed → enrolled, but require_login stays off until opted in"
        );
        // Setting the flag flips require_login on its own.
        auth::set_require_login_in(dir.path(), true).unwrap();
        assert!(
            security_state_at(dir.path()).require_login,
            "the flag drives require_login"
        );
        assert!(!s.token_set && !s.password_set, "other factors still unset");
    }

    #[test]
    fn enroll_totp_is_mint_once_with_ralphy_uri() {
        let dir = tempfile::tempdir().unwrap();
        let (uri, minted) = enroll_totp_at(dir.path()).unwrap();
        assert!(minted, "first enrol mints");
        assert!(
            uri.starts_with("otpauth://totp/ralphy:"),
            "the real provisioning URI; got {uri}"
        );
        let secret_of = |u: &str| {
            u.split("secret=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .unwrap()
                .to_string()
        };
        let (uri2, minted2) = enroll_totp_at(dir.path()).unwrap();
        assert!(!minted2, "second enrol does not re-mint");
        assert_eq!(secret_of(&uri), secret_of(&uri2), "same secret returned");
    }

    #[test]
    fn enroll_then_confirm_arms_the_live_seed() {
        let dir = tempfile::tempdir().unwrap();
        // Seed the PENDING slot with the RFC vector so we know a valid code.
        totp::save_seed_to(
            &totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
            &totp::pending_seed_path_in(dir.path()),
        )
        .unwrap();
        // Pending enrolment does not count as enrolled yet.
        assert!(
            !security_state_at(dir.path()).totp_enrolled,
            "a pending seed is not enrolled"
        );
        // A wrong code arms nothing.
        assert!(!confirm_totp_at(dir.path(), "999999", 59).unwrap());
        assert!(!security_state_at(dir.path()).totp_enrolled);
        // The RFC vector code (T=59 → 287082) confirms and arms the live seed.
        assert!(confirm_totp_at(dir.path(), "287082", 59).unwrap());
        assert!(
            security_state_at(dir.path()).totp_enrolled,
            "confirming arms TOTP"
        );
        // Revoke clears both live and (already-consumed) pending.
        revoke_totp_at(dir.path()).unwrap();
        assert!(!security_state_at(dir.path()).totp_enrolled);
    }

    #[test]
    fn set_password_round_trips_set_then_clear() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            set_password_at(dir.path(), Some("pw")).unwrap(),
            "set → true"
        );
        assert!(
            security_state_at(dir.path()).password_set,
            "state reflects the set"
        );
        assert!(!set_password_at(dir.path(), None).unwrap(), "clear → false");
        assert!(
            !security_state_at(dir.path()).password_set,
            "state reflects the clear"
        );
    }

    #[test]
    fn remint_token_yields_a_new_distinct_token() {
        let dir = tempfile::tempdir().unwrap();
        remint_token_at(dir.path()).unwrap();
        let first = auth::load_token_from(&auth::token_path_in(dir.path()))
            .unwrap()
            .expect("token written");
        assert_eq!(first.len(), 64, "64-hex token");
        remint_token_at(dir.path()).unwrap();
        let second = auth::load_token_from(&auth::token_path_in(dir.path()))
            .unwrap()
            .unwrap();
        assert_ne!(first, second, "remint rotates the token");
    }

    #[test]
    fn require_login_gate_needs_an_enrolled_seed() {
        let dir = tempfile::tempdir().unwrap();
        // Enabling with no seed is refused; disabling is always Ok.
        assert!(require_login_at(dir.path(), true).is_err());
        assert!(require_login_at(dir.path(), false).is_ok());
        // With a seed enrolled, enabling is Ok.
        totp::save_seed_to(&totp::generate_seed(), &totp::seed_path_in(dir.path())).unwrap();
        assert!(require_login_at(dir.path(), true).is_ok());
    }

    #[tokio::test]
    async fn root_serves_the_embedded_page() {
        let resp = get("/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        // Anchored on the document TITLE, not on chrome text. The old anchor was
        // the login card's "ralphy daemon" brand, which made a copy edit on one
        // screen look like a broken asset pipeline — and "daemon" left that
        // brand naming the process rather than the thing being logged into.
        assert!(
            body.contains("<title>ralphy · workbench shell</title>"),
            "the page must identify the daemon; got: {body}"
        );
    }

    #[tokio::test]
    async fn xterm_asset_is_served() {
        // The embedded xterm.js loads over HTTP with a JS content-type — the
        // terminal UI can pull it from `/vendor/xterm.js`.
        let resp = get("/vendor/xterm.js").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(!body.is_empty(), "the embedded xterm.js must be non-empty");
    }

    /// `/api/session` is allowlisted pre-login, so what it carries is what an
    /// UNAUTHENTICATED caller can read. It must carry the avatar (the login card
    /// wears this daemon's face) and it must NOT carry the name — the gate stays
    /// opaque about everything the operator wrote themselves.
    #[tokio::test]
    async fn api_session_carries_the_avatar_but_never_the_name() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/session")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("🐙"), "must carry the avatar; got: {body}");
        assert!(
            !body.contains("anvil"),
            "must NOT carry the name; got: {body}"
        );
    }

    /// An un-baptized daemon has no avatar, and `skip_serializing_if` keeps the
    /// key off the wire entirely — the SPA's `if (s.avatar)` then leaves the
    /// fallback mark in place rather than painting an empty box.
    #[tokio::test]
    async fn api_session_omits_the_avatar_when_unbaptized() {
        let body = body_string(get_local("/api/session").await).await;
        assert!(
            !body.contains("avatar"),
            "no avatar key expected; got: {body}"
        );
    }

    #[tokio::test]
    async fn unknown_path_is_404() {
        let resp = get("/no-such-asset").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_identity_route_returns_name_and_avatar() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("anvil"),
            "body must carry the name; got: {body}"
        );
        assert!(
            body.contains("🐙"),
            "body must carry the avatar; got: {body}"
        );

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn announced_descriptor_advertises_a_nudge_only_inside_wsl() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let inside = announced_descriptor(&id, 7443, Some("Ubuntu-22.04"), "tok".into());
        assert_eq!(inside.environment, "WSL: Ubuntu-22.04");
        assert_eq!(
            inside.nudge,
            Some(peer::NudgeSpec {
                distro: "Ubuntu-22.04".into(),
                unit: autostart::UNIT_NAME.into(),
            }),
            "a WSL daemon advertises how to wake it"
        );
        assert_eq!(
            inside.address, "127.0.0.1",
            "the peer transport is loopback"
        );
        assert_eq!(inside.port, 7443, "the BOUND port is announced");
        assert_eq!(inside.token, "tok", "the resolved token is announced as-is");
        assert_eq!(inside.daemon_id, id.id.to_string());

        let outside = announced_descriptor(&id, 7443, None, "tok".into());
        assert_eq!(
            outside.nudge, None,
            "no `wsl.exe` can reach a non-WSL daemon, so it advertises no nudge"
        );
        assert_ne!(outside.environment, "WSL: Ubuntu-22.04");
    }

    /// Announcing must never touch the auth policy: it takes the token it is
    /// given rather than minting over it (AC5).
    #[test]
    fn announce_skips_an_un_baptized_daemon_and_a_non_loopback_bind() {
        let dir = tempfile::tempdir().unwrap();
        let loopback = SocketAddr::from(([127, 0, 0, 1], 7443));
        announce_peer(
            &[dir.path().to_path_buf()],
            None,
            loopback,
            Some("tok".into()),
        );
        assert!(
            !dir.path().join("peers").exists(),
            "an un-baptized daemon announces nothing"
        );

        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        announce_peer(
            &[dir.path().to_path_buf()],
            Some(&id),
            SocketAddr::from(([10, 0, 0, 5], 7443)),
            Some("tok".into()),
        );
        assert!(
            !dir.path().join("peers").exists(),
            "a daemon that does not listen on loopback cannot be a peer"
        );

        // The happy path, and a SECOND store whose parent is a file — the write
        // fails there and must not stop the good one.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "not a directory").unwrap();
        announce_peer(
            &[blocked, dir.path().to_path_buf()],
            Some(&id),
            loopback,
            Some("tok-given".into()),
        );
        let written = dir
            .path()
            .join("peers")
            .join(format!("{}.toml", ulid::Ulid::nil()));
        let back: peer::PeerDescriptor =
            toml::from_str(&std::fs::read_to_string(&written).unwrap()).unwrap();
        assert_eq!(
            back.token, "tok-given",
            "the resolved token is announced, never re-minted over"
        );
    }

    #[tokio::test]
    async fn api_peer_hello_serves_the_handshake_and_404s_un_baptized() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/peer/hello")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("\"daemon_id\":\"00000000000000000000000000\""),
            "the handshake must carry the daemon_id; got: {body}"
        );
        assert!(
            body.contains("\"protocol_version\":1"),
            "the handshake must carry the peer protocol version; got: {body}"
        );
        assert!(
            body.contains("\"environment\":\"") && !body.contains("\"environment\":\"\""),
            "the handshake must name a non-empty environment; got: {body}"
        );

        // Un-baptized: 404, matching `/api/identity` — nothing for a peer to key on.
        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/peer/hello")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// Seed a scratch store with `repos.toml` plus a `peers/` descriptor pointing
    /// at a closed loopback port, and return the registry path `router` takes.
    fn seed_fleet_store(dir: &Path, peer_port: u16) -> PathBuf {
        let registry_path = dir.join("repos.toml");
        let mut store = registry::RegistryStore::default();
        store.upsert("owner/local", &dir.to_string_lossy());
        registry::save_to(&store, &registry_path).unwrap();
        peer::write_descriptor(
            dir,
            &peer::PeerDescriptor {
                daemon_id: "01PEERFAKE".into(),
                name: "wsl-box".into(),
                avatar: "🐺".into(),
                address: "127.0.0.1".into(),
                port: peer_port,
                environment: "WSL: Ubuntu-22.04".into(),
                token: "tok".into(),
                protocol_version: peer::PEER_PROTOCOL_VERSION,
                nudge: Some(peer::NudgeSpec {
                    distro: "Ubuntu-22.04".into(),
                    unit: "ralphy-daemon.service".into(),
                }),
            },
        )
        .unwrap();
        registry_path
    }

    fn fleet_router(registry_path: PathBuf) -> Router {
        router(
            Some(identity::Identity {
                id: ulid::Ulid::nil(),
                name: "anvil".into(),
                avatar: "🐙".into(),
            }),
            registry_path,
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
    }

    #[tokio::test]
    async fn api_fleet_marks_an_unreachable_peer_and_keeps_the_local_repos() {
        let dir = tempfile::tempdir().unwrap();
        // A port nothing listens on: bound to learn a free one, then dropped.
        let closed = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let registry_path = seed_fleet_store(dir.path(), closed);
        // A file that is not a descriptor at all — it must degrade to one entry,
        // never fail the route.
        std::fs::write(
            dir.path().join("peers").join("junk.toml"),
            "not toml at all",
        )
        .unwrap();

        let resp = fleet_router(registry_path)
            .oneshot(
                Request::builder()
                    .uri("/api/fleet")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();

        let peers = body["peers"].as_array().unwrap();
        assert_eq!(peers.len(), 2, "the peer AND the bad record: {peers:?}");
        let live = peers
            .iter()
            .find(|p| p["daemon_id"] == "01PEERFAKE")
            .expect("the announced peer must be listed");
        assert_eq!(live["state"], "unreachable", "got: {live}");
        assert_eq!(live["environment"], "WSL: Ubuntu-22.04");
        assert!(
            live["diagnosis"]
                .as_str()
                .unwrap()
                .contains("WSL: Ubuntu-22.04"),
            "the diagnosis must name the environment; got: {live}"
        );
        assert!(live["nudgeable"].as_bool().unwrap());
        let bad = peers
            .iter()
            .find(|p| p["state"] == "malformed")
            .expect("a fold rejection is degraded, never dropped");
        assert_eq!(bad["name"], "junk.toml");

        // Federation must never blank the local sidebar.
        let repos = body["repos"].as_array().unwrap();
        assert_eq!(repos.len(), 1, "the local row survives: {repos:?}");
        assert_eq!(repos[0]["key"], "00000000000000000000000000/owner/local");
        assert_eq!(repos[0]["peer_state"], "local");
        assert_eq!(repos[0]["local"], true);
    }

    /// The route-level proof of "marked, never removed": a peer that answered
    /// once keeps its rows listed after it stops answering, with its state
    /// changed rather than its rows dropped. Liveness is still fresh — only the
    /// repo list is remembered.
    #[tokio::test]
    async fn api_fleet_keeps_a_peers_last_known_repos_after_it_stops_answering() {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let stub = Router::new()
            .route(
                "/api/peer/hello",
                axum::routing::get(|| async {
                    Json(serde_json::json!({"protocol_version": peer::PEER_PROTOCOL_VERSION}))
                }),
            )
            .route(
                "/api/repos",
                axum::routing::get(|| async {
                    Json(serde_json::json!([{"slug": "owner/theirs", "path": "/home/p/theirs"}]))
                }),
            );
        let serving = tokio::spawn(async move {
            let _ = axum::serve(listener, stub).await;
        });

        let registry_path = seed_fleet_store(dir.path(), port);
        let app = fleet_router(registry_path);

        let fleet = |app: Router| async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .uri("/api/fleet")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()
        };

        let up = fleet(app.clone()).await;
        assert_eq!(up["peers"][0]["state"], "reachable", "got: {up}");
        let up_rows: Vec<&serde_json::Value> = up["repos"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["local"] == false)
            .collect();
        assert_eq!(up_rows.len(), 1, "the peer's repo is federated: {up}");
        assert_eq!(up_rows[0]["slug"], "owner/theirs");

        serving.abort();
        let _ = serving.await;

        let down = fleet(app.clone()).await;
        assert_eq!(
            down["peers"][0]["state"], "unreachable",
            "liveness is re-computed, never cached: {down}"
        );
        let down_rows: Vec<&serde_json::Value> = down["repos"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["local"] == false)
            .collect();
        assert_eq!(
            down_rows.len(),
            1,
            "the peer is MARKED, not removed — its last-known repos stay listed: {down}"
        );
        assert_eq!(down_rows[0]["peer_state"], "unreachable");
        assert_eq!(down_rows[0]["reachable"], false);
    }

    #[tokio::test]
    async fn api_fleet_nudge_404s_an_unknown_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let registry_path = seed_fleet_store(dir.path(), 7257);
        let resp = fleet_router(registry_path)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/fleet/nudge?daemon_id=01NOSUCHPEER")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_fleet_nudge_refuses_a_peer_that_announced_no_way_to_wake_it() {
        let dir = tempfile::tempdir().unwrap();
        let registry_path = seed_fleet_store(dir.path(), 7257);
        // Re-announce the same peer WITHOUT a nudge spec.
        peer::write_descriptor(
            dir.path(),
            &peer::PeerDescriptor {
                daemon_id: "01PEERFAKE".into(),
                name: "wsl-box".into(),
                avatar: "🐺".into(),
                address: "127.0.0.1".into(),
                port: 7257,
                environment: "WSL: Ubuntu-22.04".into(),
                token: "tok".into(),
                protocol_version: peer::PEER_PROTOCOL_VERSION,
                nudge: None,
            },
        )
        .unwrap();

        let resp = fleet_router(registry_path)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/fleet/nudge?daemon_id=01PEERFAKE")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("loginctl enable-linger"),
            "the refusal must name the prerequisite no nudge can substitute for; got: {body}"
        );
    }

    #[tokio::test]
    async fn api_about_route_reports_version_and_facts() {
        let resp = get("/api/about").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        // The build-embedded version string (git tag or Cargo fallback) — never empty.
        assert!(
            body.contains("\"version\":\"") && !body.contains("\"version\":\"\""),
            "about must carry a non-empty version; got: {body}"
        );
        assert!(
            body.contains("GPL-3.0"),
            "about must carry the license; got: {body}"
        );
        assert!(
            body.contains("Paulo Corcino"),
            "about must carry the creator; got: {body}"
        );
    }

    #[tokio::test]
    async fn api_agents_serves_the_roster() {
        let resp = get("/api/agents").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(rows.len(), session::Agent::ALL.len());
        let served: std::collections::BTreeSet<String> = rows
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect();
        let expected: std::collections::BTreeSet<String> = [
            "claude", "codex", "opencode", "kimi", "copilot", "cursor", "gemini",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(served, expected, "served roster ids: {served:?}");
        assert_eq!(
            rows[0],
            serde_json::json!({ "id": "claude", "label": "claude", "accelerator": "1" }),
            "the roster is served in accelerator order, claude first"
        );
    }

    #[tokio::test]
    async fn api_repos_reports_reachability() {
        // Write a temp repos.toml with one existing-dir entry (reachable) and one
        // bogus-path entry (unreachable), then read it back through the route.
        let dir = tempfile::tempdir().unwrap();
        let registry_path = dir.path().join("repos.toml");
        let mut store = registry::RegistryStore::default();
        store.upsert("owner/here", &dir.path().to_string_lossy());
        store.upsert("owner/gone", "/no/such/path/exists");
        registry::save_to(&store, &registry_path).unwrap();

        let resp = router(
            None,
            registry_path,
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/repos")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("owner/here") && body.contains("owner/gone"),
            "body must carry both slugs; got: {body}"
        );
        assert!(
            body.contains("\"reachable\":true"),
            "the existing-dir entry must be reachable; got: {body}"
        );
        assert!(
            body.contains("\"reachable\":false"),
            "the bogus-path entry must be unreachable; got: {body}"
        );
    }

    #[tokio::test]
    async fn api_repos_reports_branch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git").join("HEAD"),
            "ref: refs/heads/feat/mini-ide\n",
        )
        .unwrap();
        let registry_path = dir.path().join("repos.toml");
        let mut store = registry::RegistryStore::default();
        store.upsert("owner/here", &dir.path().to_string_lossy());
        store.upsert("owner/gone", "/no/such/path/exists");
        registry::save_to(&store, &registry_path).unwrap();

        let resp = router(
            None,
            registry_path,
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/repos")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("\"branch\":\"feat/mini-ide\""),
            "the reachable repo's branch must be reported; got: {body}"
        );
        assert!(
            body.contains("\"branch\":null"),
            "the unreachable repo's branch must be null; got: {body}"
        );
    }

    #[tokio::test]
    async fn api_repos_reports_dirty_and_remote() {
        fn git(dir: &std::path::Path, args: &[&str]) {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .expect("git (CI and the build machine have git)");
        }

        // (a) a dirty repo (untracked file) WITH an origin remote.
        let dirty = tempfile::tempdir().unwrap();
        git(dirty.path(), &["init"]);
        git(
            dirty.path(),
            &["remote", "add", "origin", "https://github.com/o/r.git"],
        );
        std::fs::write(dirty.path().join("untracked.txt"), "x").unwrap();
        // (b) a clean repo with NO remote.
        let clean = tempfile::tempdir().unwrap();
        git(clean.path(), &["init"]);

        let reg = tempfile::tempdir().unwrap();
        let registry_path = reg.path().join("repos.toml");
        let mut store = registry::RegistryStore::default();
        store.upsert("owner/dirty", &dirty.path().to_string_lossy());
        store.upsert("owner/clean", &clean.path().to_string_lossy());
        registry::save_to(&store, &registry_path).unwrap();

        let resp = router(
            None,
            registry_path,
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/repos")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("\"dirty\":true"),
            "the untracked-file repo must be dirty; got: {body}"
        );
        assert!(
            body.contains("\"dirty\":false"),
            "the clean repo must not be dirty; got: {body}"
        );
        assert!(
            body.contains("\"remote\":\"https://github.com/o/r.git\""),
            "the origin url must be reported; got: {body}"
        );
        assert!(
            body.contains("\"remote\":null"),
            "the remoteless repo must report null; got: {body}"
        );
    }

    #[tokio::test]
    async fn api_usage_serves_run_records_and_honors_since() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("owner-repo.jsonl"),
            "{\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"plan\",\"agent\":\"a\",\"model\":\"m\",\"session_id\":\"sess-a\",\"outcome\":\"ok\",\"tokens\":{\"input\":10,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
             {\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"m\",\"session_id\":\"sess-b\",\"outcome\":\"ok\",\"tokens\":{\"input\":20,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
        )
        .unwrap();

        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            dir.path().to_path_buf(),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("\"sess-a\""),
            "must carry sess-a; got: {body}"
        );
        assert!(
            body.contains("\"sess-b\""),
            "must carry sess-b; got: {body}"
        );
        assert!(
            body.contains("00000000000000000000000000"),
            "must carry the daemon_id; got: {body}"
        );
        assert!(
            !body.contains("usd"),
            "must not carry a usd field; got: {body}"
        );
        assert!(
            !body.contains("cost"),
            "must not carry a cost field; got: {body}"
        );

        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            dir.path().to_path_buf(),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage?since=2026-06-15T12:05:00%2B00:00")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("\"sess-b\"") && !body.contains("\"sess-a\""),
            "since must keep only sess-b; got: {body}"
        );
    }

    /// `/api/usage` now carries an `interactive` array from the Claude scan
    /// alongside the ledger's `records`. A session the ledger already owns
    /// (`run-sess`) is excluded from `interactive`; a genuinely-interactive one
    /// (`int-sess`) appears. The scan runs against a temp store, so no operator
    /// state is read.
    #[tokio::test]
    async fn api_usage_carries_run_and_interactive_records() {
        let usage_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            usage_dir.path().join("owner-repo.jsonl"),
            "{\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"plan\",\"session_id\":\"run-sess\",\"ts\":\"2026-06-15T12:00:00+00:00\"}\n",
        )
        .unwrap();

        let claude_dir = tempfile::tempdir().unwrap();
        let ws = claude_dir.path().join("ws-key");
        std::fs::create_dir_all(&ws).unwrap();
        let line = |id: &str| {
            format!(
                "{{\"requestId\":\"r1\",\"timestamp\":\"2026-07-10T10:00:00Z\",\"message\":{{\"id\":\"{id}\",\"model\":\"claude-opus-4-8\",\"usage\":{{\"input_tokens\":10,\"output_tokens\":1,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":0}}}}}}"
            )
        };
        std::fs::write(ws.join("run-sess.jsonl"), line("m1")).unwrap();
        std::fs::write(ws.join("int-sess.jsonl"), line("m2")).unwrap();

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            usage_dir.path().to_path_buf(),
            StorePaths {
                claude_projects_dir: claude_dir.path().to_path_buf(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();

        let interactive = body["interactive"].as_array().expect("interactive array");
        let has = |sid: &str| {
            interactive
                .iter()
                .any(|r| r.get("session_id").and_then(|v| v.as_str()) == Some(sid))
        };
        assert!(
            has("int-sess"),
            "interactive must carry int-sess; got: {body_string}"
        );
        assert!(
            !has("run-sess"),
            "the run-owned session must be excluded; got: {body_string}"
        );

        let records = body["records"].as_array().expect("records array");
        assert!(
            records
                .iter()
                .any(|r| r.get("session_id").and_then(|v| v.as_str()) == Some("run-sess")),
            "records must still carry the run line; got: {body_string}"
        );
        assert!(
            !body_string.contains("usd"),
            "no pricing in the payload; got: {body_string}"
        );

        // A vendor that writes every token to disk reports a total, not a floor:
        // a blanket `lower_bound: true` would mislabel the whole modal.
        let claude = interactive
            .iter()
            .find(|r| r.get("session_id").and_then(|v| v.as_str()) == Some("int-sess"))
            .unwrap();
        assert_eq!(claude["lower_bound"].as_bool(), Some(false), "{claude}");
    }

    /// `/api/usage` also carries Codex interactive records: a rollout under the
    /// codex base dir's `sessions/` tree flows through the scan and appears in the
    /// `interactive` array with `agent=="codex"` and its `session_meta.id`. Proves
    /// the codex_dir router arg is threaded end-to-end, not just Claude.
    #[tokio::test]
    async fn api_usage_carries_codex_interactive_records() {
        let codex_dir = tempfile::tempdir().unwrap();
        let roll = codex_dir
            .path()
            .join("sessions")
            .join("2026")
            .join("07")
            .join("10");
        std::fs::create_dir_all(&roll).unwrap();
        let meta_id = "019c5131-651b-78f2-b8e7-93995bff4dad";
        let body = format!(
            "{{\"timestamp\":\"2026-07-10T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{meta_id}\",\"cwd\":\"c:\\\\Dev\\\\x\"}}}}\n\
             {{\"timestamp\":\"2026-07-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{{\"model\":\"gpt-5.3-codex\"}}}}\n\
             {{\"timestamp\":\"2026-07-10T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":1000,\"cached_input_tokens\":800,\"output_tokens\":200}}}}}}}}\n"
        );
        std::fs::write(roll.join("rollout-int-abc.jsonl"), body).unwrap();

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths {
                codex_dir: codex_dir.path().to_path_buf(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let interactive = body["interactive"].as_array().expect("interactive array");
        assert!(
            interactive.iter().any(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some("codex")
                    && r.get("session_id").and_then(|v| v.as_str()) == Some(meta_id)
            }),
            "interactive must carry a codex record with the meta id; got: {body_string}"
        );
        assert!(
            !body_string.contains("usd"),
            "no pricing in the payload; got: {body_string}"
        );
    }

    /// `/api/usage` also carries OpenCode interactive records: an assistant row in
    /// a seeded `opencode.db` flows through the scan and appears in the
    /// `interactive` array with `agent=="opencode"` and its `session_id`. Proves
    /// the `opencode_db` router arg is threaded end-to-end.
    #[tokio::test]
    async fn api_usage_carries_opencode_interactive_records() {
        use rusqlite::Connection;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("opencode.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute(
                "CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)",
                [],
            )
            .unwrap();
            conn.execute("CREATE TABLE session (id TEXT, directory TEXT)", [])
                .unwrap();
            let data = r#"{"role":"assistant","modelID":"k2p6","tokens":{"input":2168,"output":100,"cache":{"write":0,"read":11264}}}"#;
            conn.execute(
                "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
                rusqlite::params!["msg_1", "ses_oc", data],
            )
            .unwrap();
        }

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths {
                opencode_db: db.clone(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let interactive = body["interactive"].as_array().expect("interactive array");
        assert!(
            interactive.iter().any(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some("opencode")
                    && r.get("session_id").and_then(|v| v.as_str()) == Some("ses_oc")
            }),
            "interactive must carry an opencode record with the session id; got: {body_string}"
        );
        assert!(
            !body_string.contains("usd"),
            "no pricing in the payload; got: {body_string}"
        );
    }

    /// `/api/usage` also carries Copilot interactive records: a row in a seeded
    /// `session-store.db` flows through the scan and appears in the `interactive`
    /// array with `agent=="copilot"` and its `session_id`. Proves the `copilot_db`
    /// router arg is threaded end-to-end.
    #[tokio::test]
    async fn api_usage_carries_copilot_interactive_records() {
        use rusqlite::Connection;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("session-store.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute(
                "CREATE TABLE assistant_usage_events (id INTEGER PRIMARY KEY AUTOINCREMENT, \
                 session_id TEXT, model TEXT, input_tokens INTEGER, output_tokens INTEGER, \
                 cache_read_tokens INTEGER, cache_write_tokens INTEGER, created_at TEXT)",
                [],
            )
            .unwrap();
            conn.execute("CREATE TABLE sessions (id TEXT PRIMARY KEY, cwd TEXT)", [])
                .unwrap();
            conn.execute(
                "INSERT INTO assistant_usage_events (session_id, model, input_tokens, \
                 output_tokens, cache_read_tokens, cache_write_tokens, created_at) \
                 VALUES ('ses_cp', 'claude-sonnet-5', 22913, 350, 0, 22903, \
                 '2026-07-20T11:54:33.066Z')",
                [],
            )
            .unwrap();
        }

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths {
                copilot_db: db.clone(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let interactive = body["interactive"].as_array().expect("interactive array");
        assert!(
            interactive.iter().any(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some("copilot")
                    && r.get("session_id").and_then(|v| v.as_str()) == Some("ses_cp")
            }),
            "interactive must carry a copilot record with the session id; got: {body_string}"
        );
        assert!(
            !body_string.contains("usd"),
            "no pricing in the payload; got: {body_string}"
        );
    }

    /// `/api/usage` also carries Kimi interactive records: a legacy `wire.jsonl`
    /// with one non-zero `StatusUpdate` under the kimi base dir's `sessions/` tree
    /// flows through the scan and appears in the `interactive` array with
    /// `agent=="kimi"` and its parent-dir session id. Proves the `kimi_dir` router
    /// arg is threaded end-to-end.
    #[tokio::test]
    async fn api_usage_carries_kimi_interactive_records() {
        let kimi_dir = tempfile::tempdir().unwrap();
        let sess = kimi_dir.path().join("sessions").join("GRP").join("SESS");
        std::fs::create_dir_all(&sess).unwrap();
        let line = "{\"timestamp\": 1770983410.0, \"message\": {\"type\": \"StatusUpdate\", \"payload\": {\"token_usage\": {\"input_other\": 100, \"output\": 10, \"input_cache_read\": 0, \"input_cache_creation\": 0}, \"message_id\": \"m1\"}}}";
        std::fs::write(sess.join("wire.jsonl"), line).unwrap();

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths {
                kimi_dir: kimi_dir.path().to_path_buf(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let interactive = body["interactive"].as_array().expect("interactive array");
        assert!(
            interactive.iter().any(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some("kimi")
                    && r.get("session_id").and_then(|v| v.as_str()) == Some("SESS")
            }),
            "interactive must carry a kimi record with the session id; got: {body_string}"
        );
        assert!(
            !body_string.contains("usd"),
            "no pricing in the payload; got: {body_string}"
        );
    }

    /// `/api/usage` also carries Cursor interactive records, and their `tokens` is
    /// JSON `null` — the key is PRESENT and is not `0` (ADR-0042 D11: Cursor keeps
    /// no token count anywhere, so "unavailable" must not serialize as "spent
    /// nothing"). Proves the `cursor_dir` router arg is threaded end-to-end.
    #[tokio::test]
    async fn api_usage_carries_cursor_interactive_records() {
        let cursor_dir = tempfile::tempdir().unwrap();
        let sid = "33333333-3333-3333-3333-333333333333";
        let sess = cursor_dir.path().join("chats").join("aaaa").join(sid);
        std::fs::create_dir_all(&sess).unwrap();
        std::fs::write(
            sess.join("meta.json"),
            r#"{"schemaVersion":1,"createdAtMs":1784593842510,"hasConversation":true,"updatedAtMs":1784593855173,"cwd":"C:\\Dev\\FinCal"}"#,
        )
        .unwrap();

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths {
                cursor_dir: cursor_dir.path().to_path_buf(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let interactive = body["interactive"].as_array().expect("interactive array");
        let record = interactive
            .iter()
            .find(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some("cursor")
                    && r.get("session_id").and_then(|v| v.as_str()) == Some(sid)
            })
            .unwrap_or_else(|| panic!("no cursor record for {sid}; got: {body_string}"));
        assert!(
            record.get("tokens").is_some(),
            "the tokens key must be PRESENT, not omitted; got: {record}"
        );
        assert!(
            record["tokens"].is_null(),
            "tokens must be null (unavailable), never 0; got: {record}"
        );
    }

    /// `/api/usage` also carries Gemini interactive records, with REAL counts —
    /// unlike Cursor's `null`, the Gemini store keeps per-turn tokens (a lower
    /// bound, ADR-0043 D10). Proves the `gemini_dir` router arg is threaded
    /// end-to-end.
    #[tokio::test]
    async fn api_usage_carries_gemini_interactive_records() {
        let gemini_dir = tempfile::tempdir().unwrap();
        let chats = gemini_dir.path().join("tmp").join("fincal").join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(
            gemini_dir
                .path()
                .join("tmp")
                .join("fincal")
                .join(".project_root"),
            "c:\\dev\\fincal",
        )
        .unwrap();
        std::fs::write(
            chats.join("session-x.jsonl"),
            "{\"sessionId\":\"ralphy-probe-p1p2p3p4p6\",\"startTime\":\"2026-07-21T00:56:00Z\",\"lastUpdated\":\"2026-07-21T01:00:00Z\",\"kind\":\"main\"}\n\
             {\"id\":\"78d80d17\",\"type\":\"gemini\",\"content\":\"OK\",\"tokens\":{\"input\":20637,\"output\":30,\"cached\":0,\"thoughts\":257,\"tool\":0,\"total\":20924},\"model\":\"gemini-3.1-pro-preview-customtools\"}\n",
        )
        .unwrap();

        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths {
                gemini_dir: gemini_dir.path().to_path_buf(),
                ..Default::default()
            },
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let raw = resp.into_body().collect().await.unwrap().to_bytes();
        let body_string = String::from_utf8_lossy(&raw);
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let interactive = body["interactive"].as_array().expect("interactive array");
        let record = interactive
            .iter()
            .find(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some("gemini")
                    && r.get("session_id").and_then(|v| v.as_str())
                        == Some("ralphy-probe-p1p2p3p4p6")
            })
            .unwrap_or_else(|| panic!("no gemini record; got: {body_string}"));
        // `total - input`, not the bare `output` field — the arithmetic survives
        // the whole route, not just the scan's own unit test.
        assert_eq!(record["tokens"]["output"].as_u64(), Some(287), "{record}");
        assert_eq!(record["tokens"]["input"].as_u64(), Some(20637), "{record}");
        // ADR-0043 D10: the served record must carry the floor label, or the UI
        // has nothing to render `≥ n (lower bound)` from.
        assert_eq!(record["lower_bound"].as_bool(), Some(true), "{record}");
    }

    #[test]
    fn build_presence_carries_identity_and_uptime() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let frame = build_presence(Some(&id), Duration::from_secs(5));
        match frame {
            Frame::Presence(p) => {
                assert_eq!(p.name, Some("anvil".into()));
                assert_eq!(p.avatar, Some("🐙".into()));
                assert_eq!(p.uptime_secs, 5);
            }
            other => panic!("expected a presence frame, got {other:?}"),
        }
    }

    #[test]
    fn bind_addr_default_is_loopback() {
        let addr = bind_addr(Ipv4Addr::LOCALHOST.into(), DEFAULT_PORT);
        assert!(addr.ip().is_loopback(), "default bind must be 127.0.0.1");
        assert_eq!(addr.port(), DEFAULT_PORT);
    }

    /// A router under a `Bearer` policy rejects a request with no
    /// `Authorization` header — the guard covers the API surface, not just `/ws`.
    #[tokio::test]
    async fn bearer_policy_rejects_missing_header() {
        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::fixed(
                auth::AuthPolicy::Bearer("tok".into()),
                epoch::SessionEpoch::in_memory_detached(),
            ),
        )
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// The same router passes a request carrying the correct bearer token.
    #[tokio::test]
    async fn bearer_policy_accepts_correct_header() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::fixed(
                auth::AuthPolicy::Bearer("tok".into()),
                epoch::SessionEpoch::in_memory_detached(),
            ),
        )
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .header(header::AUTHORIZATION, "Bearer tok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// A `Localhost` policy serves the API with no `Authorization` header.
    #[tokio::test]
    async fn localhost_policy_serves_without_token() {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// A Bearer router with the WRONG token returns `401` — the guard checks the
    /// token VALUE, not merely the header's presence (a presence-only bug would
    /// pass every other test here).
    #[tokio::test]
    async fn bearer_policy_rejects_wrong_token() {
        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::fixed(
                auth::AuthPolicy::Bearer("tok".into()),
                epoch::SessionEpoch::in_memory_detached(),
            ),
        )
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .header(header::AUTHORIZATION, "Bearer wrong")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// The RFC 6238 seed, wrapped for the session router tests.
    fn rfc_seed() -> totp::Seed {
        totp::Seed::from_bytes(b"12345678901234567890".to_vec())
    }

    /// Build a router under a `Session` policy over `token` + the RFC seed, with a
    /// baptized identity so `/api/identity` answers `200` once authorized.
    fn session_router(token: &str) -> Router {
        // One shared epoch: the `SessionAuth` signs/verifies cookies under it and
        // the wrapping `AuthState` bumps the SAME counter on logout/invalidate.
        let session_epoch = epoch::SessionEpoch::in_memory_detached();
        let policy = auth::AuthPolicy::Session(std::sync::Arc::new(auth::SessionAuth {
            token: token.to_string(),
            totp: rfc_seed(),
            password: None,
            epoch: session_epoch.clone(),
        }));
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        router(
            Some(id),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::fixed(policy, session_epoch),
        )
    }

    /// The full browser-login round trip under a `Session` policy (issue #179,
    /// promoted in #200): no-cookie `401` on data, the SPA shell is served ungated
    /// at `/` (it renders its own login gate), a valid-TOTP `POST /api/login` `200`
    /// + `Set-Cookie`, the cookie authorizes a follow-up, and a machine `Bearer`
    /// still authorizes. Plumbing only — the code itself is pinned by the `totp`
    /// RFC-vector unit test.
    #[tokio::test]
    async fn session_policy_login_flow() {
        // 1. No cookie / no bearer → the API is 401.
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/identity")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "no cookie → 401");

        // 2. The shell (which hosts its own login gate) is served without a cookie.
        let resp = session_router("tok")
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "GET / → 200 (ungated shell)");

        // 3. A valid current TOTP mints a session cookie.
        let now = now_unix();
        let code = rfc_seed().code_at(now / 30);
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!("code={code}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "valid TOTP → 200");
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("a Set-Cookie header")
            .to_string();
        assert!(
            set_cookie.contains("ralphy_session="),
            "cookie name: {set_cookie}"
        );
        assert!(set_cookie.contains("HttpOnly"), "HttpOnly: {set_cookie}");
        assert!(
            set_cookie.contains("SameSite=Strict"),
            "SameSite: {set_cookie}"
        );

        // The cookie value is everything up to the first attribute `;`.
        let cookie_pair = set_cookie.split(';').next().unwrap().to_string();

        // 4. That cookie authorizes a follow-up request.
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/identity")
                    .header(header::COOKIE, &cookie_pair)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "cookie authorizes → 200");

        // 5. The machine path is unchanged: a correct Bearer still authorizes.
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/identity")
                    .header(header::AUTHORIZATION, "Bearer tok")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "Bearer authorizes under Session"
        );
    }

    /// Read the body of a response as a lossy UTF-8 string.
    async fn body_string(resp: Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// GET a path on a Localhost router (the `get` helper above), returning the
    /// response for further assertions.
    async fn get_local(path: &str) -> Response {
        get(path).await
    }

    #[tokio::test]
    async fn root_serves_workbench_shell() {
        let resp = get_local("/").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET / → 200");
        let body = body_string(resp).await;
        assert!(
            body.contains(r#"x-data="shell()""#),
            "the workbench shell HTML must render at the root; got: {}",
            &body[..body.len().min(200)]
        );
        let resp = get_local("/app.js").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET /app.js → 200");
    }

    #[tokio::test]
    async fn root_serves_vendored_xterm() {
        let resp = get_local("/vendor/xterm.js").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET vendor/xterm.js → 200");
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        let resp = get_local("/vendor/xterm.css").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET vendor/xterm.css → 200");
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );

        let shell = body_string(get_local("/").await).await;
        assert!(
            shell.contains("vendor/xterm.js"),
            "the shell HTML must load the vendored xterm"
        );

        let console = body_string(get_local("/wb-console.js").await).await;
        assert!(
            console.contains("new Terminal("),
            "wb-console.js must construct a real xterm terminal"
        );
        assert!(
            console.contains("/ws/session"),
            "wb-console.js must open the session WebSocket"
        );
    }

    #[tokio::test]
    async fn root_serves_wb_daemon() {
        let resp = get_local("/wb-daemon.js").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET wb-daemon.js → 200");
        let daemon = body_string(resp).await;
        assert!(
            daemon.contains("ACTION_TO_VERB"),
            "wb-daemon.js must ship the action→verb map"
        );
        assert!(
            daemon.contains("/ws/command"),
            "wb-daemon.js must open the command WebSocket"
        );

        let shell = body_string(get_local("/").await).await;
        assert!(
            shell.contains("wb-daemon.js"),
            "the shell HTML must load the daemon adapter"
        );
    }

    #[tokio::test]
    async fn root_serves_wb_mode() {
        let resp = get_local("/wb-mode.js").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET wb-mode.js → 200");
        let mode = body_string(resp).await;
        assert!(
            mode.contains("function modeFor"),
            "wb-mode.js must ship the pure mode predicate"
        );

        let shell = body_string(get_local("/").await).await;
        assert!(
            shell.contains("wb-mode.js"),
            "the shell HTML must load the mode module"
        );
    }

    #[tokio::test]
    async fn favicon_is_served_for_both_pages_and_the_bare_request() {
        // The browser asks for /favicon.ico on its own, whatever the markup says,
        // so the .ico must exist even though the SVG is what a modern browser
        // picks. A 404 here is the console error this replaced.
        let resp = get_local("/favicon.ico").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET /favicon.ico → 200");
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/x-icon"
        );
        let resp = get_local("/favicon.svg").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET /favicon.svg → 200");
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/svg+xml"
        );
        for page in ["/index.html", "/detached.html", "/detached-fence.html"] {
            let html = body_string(get_local(page).await).await;
            assert!(
                html.contains("favicon.svg") && html.contains("favicon.ico"),
                "{page} must link both favicon forms"
            );
        }
    }

    #[tokio::test]
    async fn served_ui_copy_has_no_mock_or_false_claims() {
        const FILES: &[&str] = &[
            "/index.html",
            "/detached.html",
            "/detached-fence.html",
            "/app.js",
            "/styles.css",
            "/wb-console.js",
            "/wb-desk-sink.js",
            "/wb-detach-link.js",
            "/wb-daemon.js",
            "/wb-fail.js",
            "/wb-kanban.js",
            "/wb-mode.js",
            "/wb-runs.js",
            "/wb-settings.js",
            "/wb-view.js",
            "/wb-viewer.js",
        ];
        for path in FILES {
            let resp = get_local(path).await;
            assert_eq!(resp.status(), StatusCode::OK, "GET {path} → 200");
            let lc = body_string(resp).await.to_ascii_lowercase();
            assert!(!lc.contains("mock"), "{path} still contains \"mock\"");
            assert!(
                !lc.contains("nothing is written to disk"),
                "{path} still claims \"nothing is written to disk\""
            );
            assert!(
                !lc.contains("no secrets are stored"),
                "{path} still claims \"no secrets are stored\""
            );
            assert!(
                !lc.contains("any 6-digit code"),
                "{path} still claims \"any 6-digit code\""
            );
        }
    }

    #[tokio::test]
    async fn translation_is_gone_from_the_served_ui() {
        let resp = get_local("/wb-translate.js").await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "GET /wb-translate.js must 404, the module is deleted"
        );
        const FILES: &[&str] = &["/index.html", "/app.js", "/wb-viewer.js", "/styles.css"];
        for path in FILES {
            let resp = get_local(path).await;
            assert_eq!(resp.status(), StatusCode::OK, "GET {path} → 200");
            let lc = body_string(resp).await.to_ascii_lowercase();
            assert!(!lc.contains("xlate"), "{path} still contains \"xlate\"");
            assert!(
                !lc.contains("wbtranslate"),
                "{path} still contains \"wbtranslate\""
            );
        }
    }

    #[tokio::test]
    async fn consoles_tab_is_fixed_and_named() {
        let body = body_string(get_local("/app.js").await).await;
        assert!(
            !body.contains(r#"title: "Agents""#),
            "app.js must not carry the old tab title \"Agents\""
        );
        let hit = body.lines().find(|line| line.contains(r#"id: "consoles""#));
        let line = hit.unwrap_or_else(|| panic!("no line in app.js sets id: \"consoles\""));
        assert!(
            line.contains(r#"title: "Consoles""#),
            "the line setting id: \"consoles\" must also set title: \"Consoles\"; got: {line}"
        );
        assert!(
            line.contains("closable: false"),
            "the line setting id: \"consoles\" must also set closable: false; got: {line}"
        );
    }

    #[tokio::test]
    async fn root_serves_wb_fail() {
        let resp = get_local("/wb-fail.js").await;
        assert_eq!(resp.status(), StatusCode::OK, "GET wb-fail.js → 200");
        let fail = body_string(resp).await;
        assert!(
            fail.contains("function message"),
            "wb-fail.js must ship the message extractor"
        );

        let shell = body_string(get_local("/").await).await;
        assert!(
            shell.contains("wb-fail.js"),
            "the shell HTML must load the failure presenter"
        );
    }

    #[tokio::test]
    async fn session_serves_shell_but_gates_data() {
        // The shell bytes are served without a cookie…
        let resp = session_router("tok")
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "/ served pre-login (NOT redirected)"
        );

        // …but every DATA endpoint stays 401 under a no-cookie Session.
        for uri in [
            "/api/identity",
            "/ws/session?repo=x&agent=claude",
            "/ws/command",
        ] {
            let resp = session_router("tok")
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{uri} must be 401 with no cookie"
            );
        }
    }

    #[tokio::test]
    async fn session_state_reports_authed() {
        // Localhost is always authed.
        let body = body_string(get_local("/api/session").await).await;
        assert!(
            body.contains(r#""authed":true"#),
            "localhost authed: {body}"
        );

        // Session, no cookie → not authed.
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(resp).await;
        assert!(
            body.contains(r#""authed":false"#),
            "no-cookie session not authed: {body}"
        );

        // Session + a valid minted cookie → authed.
        let now = now_unix();
        let code = rfc_seed().code_at(now / 30);
        let login = session_router("tok")
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!("code={code}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        let set_cookie = login
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("a Set-Cookie header")
            .to_string();
        let cookie_pair = set_cookie.split(';').next().unwrap().to_string();
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/session")
                    .header(header::COOKIE, &cookie_pair)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(resp).await;
        assert!(
            body.contains(r#""authed":true"#),
            "valid cookie authed: {body}"
        );
    }

    /// `GET /api/session` reports the wire name of the ACTIVE policy under all
    /// three binds (issue #205), so the Security modal can derive honest,
    /// bind-specific affordances instead of always assuming `Session`.
    #[tokio::test]
    async fn session_state_reports_policy() {
        // Localhost.
        let body = body_string(get_local("/api/session").await).await;
        assert!(
            body.contains(r#""policy":"localhost""#),
            "localhost: {body}"
        );

        // Session, no cookie — the route is allowlisted (200) even though
        // `authed` is false.
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .uri("/api/session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "allowlisted: {resp:?}");
        let body = body_string(resp).await;
        assert!(body.contains(r#""policy":"session""#), "session: {body}");

        // Bearer, with a matching Authorization header.
        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::fixed(
                auth::AuthPolicy::Bearer("tok".into()),
                epoch::SessionEpoch::in_memory_detached(),
            ),
        )
        .oneshot(
            Request::builder()
                .uri("/api/session")
                .header(header::AUTHORIZATION, "Bearer tok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        let body = body_string(resp).await;
        assert!(body.contains(r#""policy":"bearer""#), "bearer: {body}");
    }

    /// The served shell no longer claims every 6-digit code works (that was
    /// true of the pre-#205 mock login) and explains the login gate inline
    /// (issue #205, audit finding AC5; copy updated for the ADR-0032 amendment
    /// where the gate can also apply to a loopback bind).
    #[tokio::test]
    async fn login_gate_drops_mock_hint() {
        let shell = body_string(get_local("/").await).await;
        assert!(
            !shell.contains("any 6-digit code works"),
            "mock hint must be gone"
        );
        assert!(
            shell.contains("Needs 2FA enrolled first"),
            "require-login explanation must be present"
        );
    }

    #[tokio::test]
    async fn logout_clears_cookie() {
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/logout")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "POST /api/logout → 200");
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("a Set-Cookie header");
        assert!(
            set_cookie.contains("ralphy_session=;") && set_cookie.contains("Max-Age=0"),
            "cookie cleared: {set_cookie}"
        );
    }

    /// A wrong TOTP code is rejected `401` by `POST /api/login` (the login handler
    /// checks the code VALUE, not merely form presence — a presence-only bug would
    /// pass the happy-path test above).
    #[tokio::test]
    async fn session_login_rejects_wrong_code() {
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("code=000000"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "wrong code → 401");
    }

    /// The auth layer covers the REMOTE-EXEC WS routes, not just `/api`: an
    /// unauthenticated `/ws/session` (PTY) and `/ws/command` (run dispatch)
    /// request is rejected `401` by the middleware BEFORE reaching the upgrade
    /// handler. A `401` here (not the handler's `400`) proves the layer fired, so
    /// a future route reordered past the layer would fail this test instead of
    /// silently serving an unauthenticated shell/run trigger.
    #[tokio::test]
    async fn bearer_policy_gates_the_remote_exec_ws_routes() {
        for uri in [
            "/ws/session?repo=x&agent=claude",
            "/ws/command",
            "/api/usage",
        ] {
            let resp = router(
                None,
                PathBuf::from("does-not-exist"),
                PathBuf::from("does-not-exist"),
                StorePaths::default(),
                Instant::now(),
                idle_shutdown(),
                auth::AuthState::fixed(
                    auth::AuthPolicy::Bearer("tok".into()),
                    epoch::SessionEpoch::in_memory_detached(),
                ),
            )
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{uri} must be gated by the auth layer, not reach its handler"
            );
        }
    }

    /// Every path embedded in [`UI`], slash-separated and relative to
    /// `assets/ui` — `include_dir` exposes only one directory level at a time.
    fn embedded_ui_paths() -> Vec<String> {
        fn walk(dir: &include_dir::Dir<'_>, out: &mut Vec<String>) {
            for f in dir.files() {
                out.push(f.path().to_string_lossy().replace('\\', "/"));
            }
            for d in dir.dirs() {
                walk(d, out);
            }
        }
        let mut out = Vec::new();
        walk(&UI, &mut out);
        out
    }

    /// #308 pins the editor swap where it can actually regress: the embedded
    /// asset tree. Monaco is vendored, CodeMirror is gone, and the four heavy
    /// language workers stay excluded (the exclusion rule is prefix-based
    /// because Monaco 0.56 ships content-hashed chunk names).
    #[test]
    fn monaco_replaced_codemirror_in_the_embedded_ui() {
        let paths = embedded_ui_paths();

        for required in [
            "vendor/monaco/vs/loader.js",
            "vendor/monaco/vs/editor/editor.main.js",
        ] {
            assert!(
                paths.iter().any(|p| p == required),
                "{required} must be embedded in the UI assets"
            );
        }

        let leftover: Vec<_> = paths
            .iter()
            .filter(|p| p.starts_with("vendor/codemirror/"))
            .collect();
        assert!(
            leftover.is_empty(),
            "CodeMirror must be deleted from the tree, found: {leftover:?}"
        );

        for worker in [
            "vs/assets/ts.worker",
            "vs/assets/css.worker",
            "vs/assets/html.worker",
            "vs/assets/json.worker",
        ] {
            let hits: Vec<_> = paths.iter().filter(|p| p.contains(worker)).collect();
            assert!(
                hits.is_empty(),
                "language worker {worker} must not be vendored, found: {hits:?}"
            );
        }

        // The other three exclusion rows of docs/WORKBENCH-BUILD-GUIDE.md, so a
        // future Monaco bump that re-copies the tarball wholesale fails here
        // rather than silently doubling the payload.
        for excluded in ["vendor/monaco/vs/language/", "vendor/monaco/vs/nls/"] {
            let hits: Vec<_> = paths.iter().filter(|p| p.starts_with(excluded)).collect();
            assert!(
                hits.is_empty(),
                "{excluded} must not be vendored, found: {hits:?}"
            );
        }
        let typed: Vec<_> = paths
            .iter()
            .filter(|p| {
                p.starts_with("vendor/monaco/") && (p.ends_with(".d.ts") || p.ends_with(".map"))
            })
            .collect();
        assert!(
            typed.is_empty(),
            "no .d.ts / .map may be vendored with Monaco, found: {typed:?}"
        );

        assert!(
            include_str!("../assets/ui/wb-monaco.js").contains("monaco.editor.create"),
            "wb-monaco.js must build the editor through Monaco's own factory"
        );

        let viewer = include_str!("../assets/ui/wb-viewer.js");
        assert!(
            viewer.contains("WBMonaco.create"),
            "wb-viewer.js must mount its editor through WBMonaco"
        );
        // Built from parts so this pin cannot trip on its own source text.
        let outgoing = concat!("Code", "Mirror(");
        assert!(
            !viewer.contains(outgoing),
            "wb-viewer.js must not construct a {outgoing} editor"
        );
    }

    /// #309's deliverable is JS/HTML no Rust gate compiles — this pin over the
    /// served assets is the same reason the #308 Monaco pin above and
    /// `usage.rs:544` exist, so `cargo test` reds after a deletion.
    #[test]
    fn the_changes_section_renders_a_status_marked_list() {
        let html = include_str!("../assets/ui/index.html");
        assert!(
            html.contains(r#"class="changes-list""#),
            "index.html must render the changes-list"
        );
        assert!(
            html.contains(r#"class="chg-mark""#),
            "index.html must render a chg-mark per row"
        );
        assert!(
            html.contains("showSideView('changes')"),
            "index.html must wire the rail's Changes button to showSideView"
        );

        let js = include_str!("../assets/ui/wb-changes.js");
        assert!(
            js.contains("st-unknown"),
            "wb-changes.js must keep the st-unknown fallback"
        );
        for status in [
            "modified",
            "added",
            "deleted",
            "renamed",
            "untracked",
            "conflicted",
        ] {
            assert!(
                js.contains(status),
                "wb-changes.js must keep the {status} marker"
            );
        }

        // The diff tab (#311) is JS/HTML only, so no other Rust gate compiles it:
        // pin the row's click wiring and the diff editor's factory here.
        assert!(
            html.contains("openDiff(openSlug, c)"),
            "index.html must open a diff from a changes row"
        );
        assert!(
            include_str!("../assets/ui/wb-viewer.js").contains("WBMonaco.createDiff"),
            "wb-viewer.js must mount the diff through WBMonaco.createDiff"
        );
        assert!(
            js.contains("diffTarget"),
            "wb-changes.js must expose diffTarget"
        );

        // The staged/unstaged split (#315) is JS/HTML too: pin the row's two
        // halves, the group headline, and the per-group `:key` prefix without
        // which Alpine collides a staged-then-modified path's two rows.
        for pin in [
            r#"class="chg-name""#,
            r#"class="chg-dir""#,
            r#"class="chg-group-head""#,
            ">Staged Changes<",
            // BOTH keys: pinning only the staged one stays green if the two
            // templates are keyed identically.
            "'s:' + c.path",
            "'u:' + c.path",
        ] {
            assert!(
                html.contains(pin),
                "index.html must keep the changes-group pin {pin}"
            );
        }
        for pin in ["worktreeStatus", "indexStatus", "lastIndexOf"] {
            assert!(
                js.contains(pin),
                "wb-changes.js must keep the index-split pin {pin}"
            );
        }

        // The promotion to a rail view (#317): the view's root, the rail icon
        // that reaches it, and the per-project indicator that keeps the count
        // reachable with no navigation.
        for pin in [
            r#"class="changes-view""#,
            r#"data-lucide="git-compare""#,
            r#"class="chg-badge""#,
        ] {
            assert!(
                html.contains(pin),
                "index.html must keep the rail-view pin {pin}"
            );
        }
        // NEGATED: the accordion is removed, not left standing beside the view.
        // Both strings occurred ONLY in the block #317 deleted, so either one
        // reappearing means the section came back.
        for gone in ["changes-sec", "toggleChanges"] {
            assert!(
                !html.contains(gone),
                "index.html must not resurrect the Changes accordion ({gone})"
            );
        }
        assert!(
            js.contains("function projectBadge("),
            "wb-changes.js must keep the per-project badge fold (#317)"
        );

        // The write controls (#318). Neither `node --test` nor a Playwright pass
        // runs in CI, so these substring pins are the ONLY CI-visible gate over
        // this markup — every control the panel's write gesture needs is named.
        for pin in [
            r#"data-act="stage""#,
            r#"data-act="unstage""#,
            r#"data-act="stage-all""#,
            r#"data-act="unstage-all""#,
            r#"class="chg-commit""#,
            r#"x-model="commitMsg""#,
            "writeLocked()",
            "commitTarget().label",
        ] {
            assert!(
                html.contains(pin),
                "index.html must keep the write-control pin {pin}"
            );
        }
        // NEGATED: the message box is no longer inert. That title string existed
        // ONLY on the disabled `.chg-msg`, so its reappearance means the box
        // regressed to a placeholder.
        assert!(
            !html.contains("inert until the write controls land"),
            "the commit message box must no longer declare itself inert (#318)"
        );
        for pin in [
            "function groupPaths(",
            "function commitTarget(",
            "function writeLockReason(",
        ] {
            assert!(
                js.contains(pin),
                "wb-changes.js must keep the write-control helper {pin}"
            );
        }
    }

    /// The discard control (#319) — the same CI-visible substring gate the write
    /// controls get, plus the NEGATED pin that keeps a group-level "discard all"
    /// out: the issue asks for one file at a time, and a one-tap discard of every
    /// path is precisely the mis-tap PRD #297 refused to ship.
    #[test]
    fn the_discard_control_is_pinned_in_the_markup() {
        let html = include_str!("../assets/ui/index.html");
        let js = include_str!("../assets/ui/wb-changes.js");

        for pin in [
            r#"data-act="discard""#,
            r#"class="chg-group-note""#,
            "groupNote('unstaged')",
            "groupNote('staged')",
            "discardRow(openSlug, c)",
        ] {
            assert!(
                html.contains(pin),
                "index.html must keep the discard pin {pin}"
            );
        }
        assert!(
            !html.contains(r#"data-act="discard-all""#),
            "there is no group-level discard: one file at a time (#319)"
        );
        // A conflicted row is all worktree work, so it lands in the UNSTAGED
        // group — but `restore --worktree` refuses an unmerged path, so the
        // control must not be offered there either.
        assert!(
            html.contains(r#"x-show="c.status !== 'conflicted'""#),
            "the discard control must be withheld from a conflicted row (#319)"
        );
        // The "unstaged rows ONLY" invariant, as a CI-VISIBLE oracle: neither
        // `node --test` nor Playwright runs in CI, so scenario 2 of
        // `wb_changes_319.py` cannot be the only thing proving it. The staged
        // list is the block between its `x-for` key and that list's close.
        let staged_from = html
            .find(r#"'s:' + c.path"#)
            .expect("index.html must keep the staged group's x-for key");
        let staged_block = &html[staged_from..];
        let staged_end = staged_block
            .find("</ul>")
            .expect("the staged group's list must close");
        assert!(
            !staged_block[..staged_end].contains(r#"data-act="discard""#),
            "the staged group must carry NO discard control — unstage comes first (#319)"
        );
        for pin in ["function discardConfirm(", "function groupDiscardNote("] {
            assert!(js.contains(pin), "wb-changes.js must keep the fold {pin}");
        }
    }

    /// The fence floor, pinned where CI can see it — neither the node table nor
    /// the Playwright suite runs there, so this is the only gate that fails when
    /// the shell half of #340 is deleted or renamed.
    #[test]
    fn shell_draws_fences_below_the_windows() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function fenceSpawnRect(",
            "function nextFenceSlot(",
            "function renderFences(",
            "function createFence(",
            "function renameFence(",
            "function removeFence(",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #340 pin {pin}"
            );
        }
        // The plane is sized to windows AND fences (ADR-0051 §2). Reverting this
        // ONE selector leaves every other test green while a fence past the last
        // window becomes unreachable — the stage never grows to hold it.
        assert!(
            js.contains(r#"querySelectorAll(".session-window, .fence")"#),
            "applyExtent must fold the fences into the stage extent (#340)"
        );
        let app = include_str!("../assets/ui/app.js");
        assert!(
            app.contains("newFence("),
            "app.js must wire the toolbar act"
        );
        let html = include_str!("../assets/ui/index.html");
        assert!(
            html.contains("newFence()"),
            "index.html must carry the Fence button (#340)"
        );

        // Scoped to the `.fence` rule's OWN body: `pointer-events` and a small
        // `z-index` both occur elsewhere in the sheet, so an unscoped substring
        // would pass with the fence tier deleted.
        let css = include_str!("../assets/ui/styles.css");
        let rule = |head: &str| -> String {
            let after = css
                .split_once(head)
                .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
                .1;
            after[..after.find('}').expect("the rule must close")].to_string()
        };
        let fence = rule("\n.fence {");
        assert!(
            fence.contains("pointer-events: none"),
            "the fence floor is inert — it may never swallow a window gesture (#340)"
        );
        // The SEMICOLON is load-bearing: a bare `z-index: 1` substring is also
        // satisfied by `10`, `100` and `1000` — a fence raised over `Z_BASE`
        // (60), which is the exact defect this pin names.
        assert!(
            fence.contains("z-index: 1;"),
            "a fence draws BELOW every console window (#340)"
        );
        // NEGATIVE CONTROL: "make the whole thing inert" would delete the rename
        // affordance, and the two pins above would still be green.
        assert!(
            rule("\n.fence-name {").contains("pointer-events: auto"),
            "the name field must stay clickable — a fence is renamed in place (#340)"
        );
        // The name is inert until it is EDITED, and an edit is cancellable. The
        // press is cancelled in `buildFence` (no focus ring, no caret) and the
        // drag half is here, so a sweep across the head cannot paint the name.
        assert!(
            rule("\n.fence-name[readonly] {").contains("user-select: none"),
            "a read-only fence name must not be selectable by a sweep"
        );
        let squeezed: String = js.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squeezed.contains(r#"name.addEventListener("mousedown", (e) => { if (name.readOnly) e.preventDefault(); });"#),
            "a single click on a read-only fence name must leave no trace at all"
        );
        // Enter is the ONLY commit. `change` fires on blur, so committing there
        // made "click away" mean SAVE — with nothing able to undo it.
        assert!(
            !js.contains(r#"name.addEventListener("change""#),
            "the fence name must not commit on `change` — leaving the field CANCELS"
        );
        assert!(
            squeezed.contains("if (!commit) name.value = pristine;"),
            "ending an edit without a commit must restore the name"
        );
        // …and the cancel must not rely on `blur`: the plane's pan handler
        // `preventDefault()`s mousedown, so pressing the stage does not move focus
        // at all (measured — the field kept its caret and the half-typed name).
        assert!(
            squeezed.contains(r#"document.addEventListener("pointerdown", stopOutside, true);"#),
            "a press outside the field must end the edit, since blur cannot be trusted here"
        );
    }

    /// A fence is a GROUP (#341): derived membership, non-overlap, and the two
    /// gestures that carry it. Same reason as the pin above — neither the node
    /// table nor the Playwright suite runs in CI, so this is the only gate that
    /// fails when this slice is deleted or renamed.
    #[test]
    fn shell_fences_are_a_group() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function fenceMembership(",
            "function fenceFits(",
            "function fenceMoveDelta(",
            "function startFenceMove(",
            "function startFenceResize(",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #341 pin {pin}"
            );
        }
        // Membership is DERIVED, never stored: the only fence id in the shell is
        // the fence element's OWN `data-fence-id`. A desk record that carried one
        // is exactly the state that can disagree with the geometry.
        let persist = js
            .split_once("function persistWin(")
            .expect("wb-console.js must keep persistWin")
            .1;
        assert!(
            !persist[..persist.find("\n  }").expect("persistWin must close")].contains("fence"),
            "no window record may carry a stored fence id (#341)"
        );
        let css = include_str!("../assets/ui/styles.css");
        let rule = |head: &str| -> String {
            let after = css
                .split_once(head)
                .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
                .1;
            after[..after.find('}').expect("the rule must close")].to_string()
        };
        // Both handles opt back IN, against a `.fence`/`.fence-head` that stay
        // inert — that pair is the whole hit-test contract of this slice. The
        // resize handle is `.fence-edge` since ADR-0051 §7a made every border a
        // handle; `.fence-grip` is the SE one's second class and no longer
        // carries the opt-in itself.
        for head in ["\n.fence-grab {", "\n.fence-edge {"] {
            assert!(
                rule(head).contains("pointer-events: auto"),
                "{head} must take pointer events — it is a gesture handle (#341)"
            );
        }
        assert!(
            rule("\n.fence-head {").contains("width: max-content"),
            "the head stays shrink-wrapped — a full-width band swallows the floor pan (#340/#341)"
        );
        // The STACKING ORDER of the head against those bands. Measured when §7a
        // landed: the bands are `position:absolute` and the head is static, so
        // they paint over it whatever the DOM order — the NW corner covered the
        // `⠿` grab and a fence MOVE silently became a resize toward the origin,
        // leaving every member behind. DOM order alone cannot express this, so
        // the lift is pinned here.
        for head in ["\n.fence-head {", "\n.fence-tools {"] {
            assert!(
                rule(head).contains("z-index: 2"),
                "{head} must sit ABOVE the resize bands, or they swallow its controls (§7a)"
            );
        }
        // The refusal must be VISIBLE: a silent revert reads as a dropped drag.
        assert!(
            rule("\n.fence-invalid {").contains("var(--danger)"),
            "a refused fence drop must show feedback, not just revert (#341)"
        );
    }

    /// Arrange moved INTO the fence (#342): the global control is retired and
    /// tiling is a per-fence act over a pure fold. Same reason as the pins
    /// above — this is the only gate that runs in CI, so a revert of either
    /// half (the retirement or the fence chrome) fails HERE or nowhere.
    #[test]
    fn shell_arranges_into_the_fence() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function tileIntoRect(",
            "function arrangeFence(",
            "function fenceRepos(",
            "function refreshFenceChrome(",
            "fence-arrange",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #342 pin {pin}"
            );
        }
        // The global act is GONE, not wrapped: a surviving entry point is a
        // second meaning of "arrange" (ADR-0051 §7). Safe against the pin above
        // — `"function arrangeFence("` does not contain `"function arrange("`.
        assert!(
            !js.contains("function arrange("),
            "the global arrange must be retired, not kept as a wrapper (#342)"
        );
        // The #338 rule — a maximized console is never tiled — has no other home
        // now that the global arrange is gone. `.maximized` overrides all four
        // offsets with `!important`, so a tile rect written onto one is
        // invisible while it silently replaces the rect the restore reads back.
        let fence_arrange = js
            .split_once("function arrangeFence(")
            .expect("wb-console.js must keep arrangeFence")
            .1;
        // The EXPRESSION, not the bare word: `"maximized"` alone is satisfied by
        // the comment that explains the rule, so deleting the filter leaves this
        // green — measured. A pin a comment can satisfy is not a pin.
        assert!(
            fence_arrange[..fence_arrange
                .find("\n  }")
                .expect("arrangeFence must close")]
                .contains(r#"!m.el.classList.contains("maximized")"#),
            "arrangeFence must exclude a maximized member from the grid (#338/#342)"
        );
        assert!(
            fence_arrange[..fence_arrange
                .find("\n  }")
                .expect("arrangeFence must close")]
                .contains("minWidth"),
            "arrangeFence must relax the CSS floor for a tile below it (#342)"
        );
        let app = include_str!("../assets/ui/app.js");
        assert!(
            !app.contains("arrangeConsoles"),
            "the shell's global arrange action must be gone (#342)"
        );
        let html = include_str!("../assets/ui/index.html");
        assert!(
            !html.contains("Arrange"),
            "the canvas toolbar must carry no Arrange control (#342)"
        );
        // The fence's arrange button opts back INTO pointer events, against a
        // `.fence`/`.fence-head` that stay inert — without this the control is
        // drawn and unclickable, and every other pin here stays green.
        let css = include_str!("../assets/ui/styles.css");
        let rule = |head: &str| -> String {
            let after = css
                .split_once(head)
                .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
                .1;
            after[..after.find('}').expect("the rule must close")].to_string()
        };
        assert!(
            rule("\n.fence-arrange {").contains("pointer-events: auto"),
            "the fence's arrange button must take pointer events (#342)"
        );
        // The window floor lives in TWO files — the CSS declaration and the
        // constants `arrangeFence` relaxes it against. Nothing else notices when
        // they diverge: the tiles would silently render past the fence again.
        for (konst, decl) in [
            ("WIN_MIN_W = 240", "min-width: 240px"),
            ("WIN_MIN_H = 150", "min-height: 150px"),
        ] {
            assert!(
                js.contains(konst),
                "wb-console.js must mirror the window floor as {konst} (#342)"
            );
            assert!(
                rule("\n.session-window {").contains(decl),
                "styles.css's window floor must still be `{decl}` — wb-console.js mirrors it (#342)"
            );
        }
    }

    /// The fence list is the MAP (#343): the toolbar picker, the jump that
    /// reuses #337's arithmetic, and the birth of a console inside the focused
    /// fence. Same reason as the pins above — neither the node table nor the
    /// Playwright suite runs in CI, so a deletion fails HERE or nowhere. Every
    /// pin below is an EXPRESSION, not a bare noun: #342 measured that a
    /// function's own explanatory comment satisfies a noun pin, leaving it green
    /// over deleted code.
    #[test]
    fn shell_lists_the_fences() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function rectHolds(",
            "function fenceSummaries(",
            "function fenceList(",
            "function jumpToFence(",
            "function spawnRectIn(",
            "function focusFence(",
            "function clearFenceFocus(",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #343 pin {pin}"
            );
        }
        let body = |name: &str| -> String {
            let after = js
                .split_once(name)
                .unwrap_or_else(|| panic!("wb-console.js must keep {name}"))
                .1;
            after[..after.find("\n  }").expect("the function must close")].to_string()
        };
        // The issue's own rule: the jump REUSES a shared fold, it does not
        // re-derive the arithmetic. Both halves are load-bearing — the call
        // site, and the fact that there is exactly one definition to call. A
        // second copy would satisfy the first assertion alone.
        //
        // ADR-0051 §7 (amended) changed WHICH fold: the jump ANCHORS the fence's
        // top-left corner rather than centring it. `bringIntoView` still exists
        // and still centres for the Go-to picker (#337), so both are pinned —
        // routing the jump back through the centring one is the regression this
        // catches.
        assert!(
            body("function jumpToFence(").contains("anchorIntoView(restoreRect(el)"),
            "jumpToFence must anchor the fence's corner through anchorIntoView (#343, §7)"
        );
        for (name, what) in [
            ("function bringIntoView(", "bring-into-view"),
            ("function anchorIntoView(", "anchor-into-view"),
        ] {
            assert_eq!(
                js.matches(name).count(),
                1,
                "there may be exactly ONE {what} implementation (#343)"
            );
        }
        // The slide is a VIEW effect layered on top, never a substitute for the
        // committed offsets: `slideTo` is cancellable, and both the floor's pan
        // and the wheel take the view back from a jump still in flight.
        assert!(
            body("function jumpToFence(").contains("slideTo(ws, to)"),
            "the jump must travel through the cancellable slide (§7)"
        );
        for owner in ["function onFloorDown(", "function onWheel("] {
            assert!(
                body(owner).contains("cancelSlide()"),
                "{owner} must abandon a slide in flight — the operator's hand outranks it (§7)"
            );
        }
        // The keyboard walk, and the rule that makes it a sweep rather than a
        // teleport: READING ORDER off the geometry, not the desk array's order.
        assert!(
            body("function fenceCycle(").contains("fenceOrder("),
            "the walk must step through the reading-order fold, not the desk array (§7)"
        );
        assert!(
            body("function stepFence(").contains("jumpToFence("),
            "a keyboard step must be a real jump, not a bare focus flip (§7)"
        );
        // The focus STATE MACHINE, not just its function names: measured, a
        // `focusFence` gutted to an empty body leaves every other pin here, the
        // whole node table and clippy green — while no ring renders and every
        // console is born free, which is this issue's headline behaviour. The
        // module variable and the class are what make focus real.
        let focus = body("function focusFence(");
        assert!(
            focus.contains("focusedFence = id") && focus.contains("is-focused"),
            "focusFence must set the module's focused id AND mark the element (#343)"
        );
        assert!(
            body("function clearFenceFocus(").contains("focusFence(null)"),
            "clearing focus must go through the same one setter (#343)"
        );
        assert!(
            body("function jumpToFence(").contains("focusFence(id)"),
            "the jump must FOCUS the fence it lands on — that is what the birth path reads (#343)"
        );
        assert!(
            body("function onFloorDown(").contains("clearFenceFocus()"),
            "a bare-floor press outside the focused fence must clear it (#343)"
        );
        // The containment predicate is REUSED, not re-spelled — the same rule
        // the issue states for `bringIntoView`. Pinning only the definition
        // lets an inlined comparison sit beside it as exported dead code.
        for (owner, what) in [
            ("function fenceMembership(", "membership"),
            ("function onFloorDown(", "the floor's focus hit test"),
        ] {
            assert!(
                body(owner).contains("rectHolds("),
                "{what} must go through the one containment predicate (#343)"
            );
        }
        // The birth site goes through the pure box, so "a console opened while a
        // fence is focused lands inside it" is arithmetic, not a hand-placed
        // rect that drifts from the fence's own geometry.
        assert!(
            body("function buildChrome(").contains("spawnRectIn("),
            "buildChrome must place a fence-born console through spawnRectIn (#343)"
        );
        // One fold feeds BOTH readouts: the fence's own chrome and the toolbar
        // row can never disagree about a count.
        assert!(
            body("function refreshFenceChrome(").contains("fenceSummaries("),
            "the fence chrome must read the same fold the list does (#343)"
        );
        let app = include_str!("../assets/ui/app.js");
        for pin in ["jumpFence(", "fenceList()"] {
            assert!(app.contains(pin), "app.js must keep the #343 pin {pin}");
        }
        let html = include_str!("../assets/ui/index.html");
        // `class="fence-item"`, not the bare noun: the markup's own comment
        // names the class, so a rename on the real element would leave a bare
        // `"fence-item"` pin green while every row loses its styling.
        for pin in ["jumpFence(", r#"class="fence-item""#] {
            assert!(
                html.contains(pin),
                "index.html must keep the #343 pin {pin}"
            );
        }
        // The focused fence must be VISIBLE — an invisible focus makes "the next
        // console is born over there" unexplainable to the operator.
        let css = include_str!("../assets/ui/styles.css");
        let rule = |head: &str| -> String {
            let after = css
                .split_once(head)
                .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
                .1;
            after[..after.find('}').expect("the rule must close")].to_string()
        };
        // A DEFINED token: an `outline` shorthand naming an undefined custom
        // property is invalid as a whole, so the ring silently never renders —
        // measured, with `--accent`, which this palette does not have.
        let focused = rule("\n.fence.is-focused {");
        assert!(
            focused.contains("outline: 2px solid var(--console-text)"),
            "the focused fence must carry a visible ring (#343)"
        );
        assert!(
            css.contains("--console-text:"),
            "the focus ring's colour token must exist — an undefined one voids the whole shorthand (#343)"
        );
    }

    /// A fence detaches into its own window, and comes home (#346). Neither the
    /// node table nor the Playwright suite runs in CI, so a deletion fails HERE
    /// or nowhere. Every pin is an EXPRESSION, not a bare noun: #342 measured
    /// that a function's own explanatory comment satisfies a noun pin, leaving
    /// it green over deleted code.
    #[test]
    fn shell_detaches_a_fence() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function detachFold(",
            "const DETACH_MAX = 4",
            "function detachFence(",
            "function reattachFence(",
            "function mountDetached(",
            "fence-detach",
            "fence-detached",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #346 pin {pin}"
            );
        }
        let body = |name: &str| -> String {
            let after = js
                .split_once(name)
                .unwrap_or_else(|| panic!("wb-console.js must keep {name}"))
                .1;
            after[..after.find("\n  }").expect("the function must close")].to_string()
        };
        // THE SINK SEAM. The persistence call sites must reach the desk through
        // the injected sink and carry NO `detached` branch of their own —
        // "incapable of writing", not "careful not to".
        assert!(
            body("function flushDesk(").contains("deskSink.put(body)"),
            "flushDesk must write through the injected sink (#346)"
        );
        assert!(
            !body("function flushDesk(").contains("detach"),
            "flushDesk must carry no detached branch — the sink is the seam (#346)"
        );
        assert!(
            js.contains("deskSink.putSync("),
            "the pagehide flush must write through the injected sink (#346)"
        );
        // The PUT literal now lives in exactly one file. The second half is the
        // NEGATIVE CONTROL: deleting the write wholesale would satisfy the first
        // assertion alone.
        assert!(
            !js.contains(r#""/api/desk", {"#),
            "the desk PUT must live only in wb-desk-sink.js (#346)"
        );
        let sink = include_str!("../assets/ui/wb-desk-sink.js");
        assert!(
            sink.contains(r#""/api/desk", {"#),
            "wb-desk-sink.js must still perform the desk PUT (#346)"
        );
        assert!(
            sink.contains("keepalive: true"),
            "the sink's putSync must outlive the closing document (#346)"
        );
        // Arrange is a no-op on a detached fence (ADR-0051 §7a): its consoles are
        // in another window, and tiling the empty box would rewrite the very
        // rects the re-attach restores from.
        assert!(
            body("function arrangeFence(").contains("if (detached.includes(id)) return;"),
            "arrangeFence must bail on a detached fence (#346, §7a)"
        );
        // Removing a detached fence would destroy the glyph that is the only way
        // home and keep its DETACH_MAX slot consumed — refuse it, do not silently
        // strand the popup.
        assert!(
            body("function removeFence(").contains("detached.includes(id)"),
            "removeFence must refuse a detached fence (#346, §7a)"
        );
        // The re-attach message must trust the SOURCE lookup, not the payload:
        // `m.fenceId` would let a popup for fence A re-attach fence B.
        assert!(
            !js.contains("reattachFence(m.fenceId"),
            "the re-attach message must use the proven owner, never its payload (#346)"
        );
        // The cap is the fold's, not a caller's: a second copy of the rule would
        // satisfy a bare-noun pin while the fold's own check was deleted.
        assert!(
            body("function detachFold(").contains("reg.length >= DETACH_MAX"),
            "the four-popup cap must be enforced inside the fold (#346)"
        );
        // The INVARIANT: either the popup exists and the members are torn down,
        // or neither. `window.open` must therefore be reached before a single
        // window is touched, and a null handle must bail.
        let detach = body("function detachFence(");
        assert!(
            detach.contains("window.open(\"detached-fence.html\""),
            "detachFence must open the popup document (#346)"
        );
        assert!(
            detach.contains("if (!handle)"),
            "a blocked popup must bail BEFORE anything is torn down (#346)"
        );
        assert!(
            detach
                .find("window.open(")
                .expect("detachFence must open a popup")
                < detach
                    .find("tearDownMember(")
                    .expect("detachFence must tear its members down"),
            "the popup must be open before a member is torn down — the #346 invariant"
        );
        // A member leaves the plane WITHOUT losing its desk record and WITHOUT
        // closing its daemon session: the record is shared state a second client
        // still renders, and the socket close is the writer-slot release (§9).
        let teardown = body("function tearDownMember(");
        assert!(
            teardown.contains("win._term?.dispose()") && teardown.contains("wins.delete(win)"),
            "tearDownMember must dispose the terminal and drop the window (#346, §9)"
        );
        assert!(
            !teardown.contains("forgetRecord") && !teardown.contains("sessions/close"),
            "a detached member must keep its desk record and its daemon session (#346)"
        );
        // THE POPUP'S DENIED CAPABILITIES. Each is what stops a window holding a
        // FRAGMENT of the plane from writing the whole desk or the shell's view.
        let html = include_str!("../assets/ui/detached-fence.html");
        for pin in [
            "window.WBDeskSink.none()",
            "autoBoot: false",
            "canLaunch: false",
            "read: () => null",
            "WBConsole.mountDetached(",
            "wb-fence-ready",
            "wb-fence-reattach",
        ] {
            assert!(
                html.contains(pin),
                "detached-fence.html must keep the #346 pin {pin}"
            );
        }
        assert!(
            !html.contains("WBConsole.open("),
            "the popup must expose no way to open a new console (#346)"
        );
        // The handshake's confidentiality control: a concrete targetOrigin, so a
        // page on any other origin never receives this fence's members. Pinned as
        // the ASSIGNMENT, not the bare noun — `window.location.origin` also
        // appears in the inbound guard below it, so rewriting `PEER` to an
        // unconditional `"*"` would leave a noun pin green (the file already
        // contains a literal `"*"` for the demo leg).
        assert!(
            html.contains(": window.location.origin;"),
            "the popup's PEER must resolve to a concrete origin (#346)"
        );
        assert!(
            !html.contains(r#"postMessage({ type: "wb-fence-ready" }, "*")"#),
            "the popup must never broadcast its handshake to \"*\" (#346)"
        );
        // The opener's reply is demo-aware for the same reason the popup's PEER
        // is: under `file://` an unconditional `location.origin` is dropped.
        assert!(
            body("window.addEventListener(\"message\"")
                .contains("isDemo() ? \"*\" : location.origin"),
            "the opener's handover must mirror the popup's demo-aware origin (#346)"
        );
        // The tree-wide sweep in `shell_stores_only_the_view_in_the_browser`
        // scans .html too; keep this document out of the browser's stores.
        assert!(
            !html.contains("localStorage") && !html.contains("sessionStorage"),
            "the popup must store nothing in the browser (#346)"
        );
        // `.fence-tools` and `.fence` are transparent to pointer events, so a
        // control that does not opt back IN is drawn and unclickable — while
        // every source-text pin above stays green. Measured in #342.
        let css = include_str!("../assets/ui/styles.css");
        let rule = |head: &str| -> String {
            let after = css
                .split_once(head)
                .unwrap_or_else(|| panic!("styles.css must keep the {head} rule"))
                .1;
            after[..after.find('}').expect("the rule must close")].to_string()
        };
        for head in ["\n.fence-detach {", "\n.fence-detached {"] {
            assert!(
                rule(head).contains("pointer-events: auto"),
                "{head} must take pointer events, or the control is inert (#346)"
            );
        }
        // SCRIPT ORDER, the same hazard #339 pinned for wb-view.js. `wb-console.js`
        // hard-dereferences `window.WBDeskSink.daemon()` at module load, so a
        // reordered or dropped tag throws out of the whole IIFE and the Consoles
        // tab dies — while every source-text pin above stays green.
        let shell = include_str!("../assets/ui/index.html");
        for (doc, name) in [(shell, "index.html"), (html, "detached-fence.html")] {
            let sink_tag = doc
                .find(r#"<script src="wb-desk-sink.js"></script>"#)
                .unwrap_or_else(|| panic!("{name} must load wb-desk-sink.js (#346)"));
            let console_tag = doc
                .find(r#"<script src="wb-console.js"></script>"#)
                .unwrap_or_else(|| panic!("{name} must load wb-console.js (#346)"));
            assert!(
                sink_tag < console_tag,
                "{name}: wb-desk-sink.js must be script-tagged BEFORE wb-console.js (#346)"
            );
        }
    }

    /// The detach survives an F5, and dies with the tab that opened it (#347).
    /// Same bargain as `shell_detaches_a_fence`: neither the node table nor the
    /// Playwright suite runs in CI, so a deletion fails HERE or nowhere. Every
    /// pin is an EXPRESSION — #342 measured that a function's own explanatory
    /// comment satisfies a bare-noun pin over deleted code.
    #[test]
    fn shell_survives_a_reload_with_its_detach() {
        let js = include_str!("../assets/ui/wb-console.js");
        let link = include_str!("../assets/ui/wb-detach-link.js");
        let html = include_str!("../assets/ui/detached-fence.html");
        let shell = include_str!("../assets/ui/index.html");
        let body = |name: &str| -> String {
            let after = js
                .split_once(name)
                .unwrap_or_else(|| panic!("wb-console.js must keep {name}"))
                .1;
            after[..after.find("\n  }").expect("the function must close")].to_string()
        };

        // The rule is a PURE FOLD, and the shell reaches storage and channel
        // only through the injected link.
        for pin in [
            "function peerFold(",
            "link.readRegistry()",
            "link.writeRegistry(",
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #347 pin {pin}"
            );
        }
        // THE STORE SEAM, with its NEGATIVE CONTROL: the console module must name
        // no browser store at all, and the link module must be the one that does
        // — deleting the store wholesale has to be red, not green.
        assert!(
            !js.contains("sessionStorage"),
            "wb-console.js must reach the registry only through the injected link (#347)"
        );
        // The CALLS, not the bare nouns: this file's own prose names
        // `sessionStorage` three times, so a noun pin stays green over
        // no-op'd `readRecord`/`writeRecord` bodies — the exact defect the
        // doc comment above warns about.
        assert!(
            link.contains("sessionStorage.getItem(KEY)")
                && link.contains("sessionStorage.setItem(KEY,")
                && link.contains("new BroadcastChannel(CHANNEL)"),
            "wb-detach-link.js IS the registry and the channel — deleting it is not how #347 stays green"
        );
        assert!(
            link.contains(r#"const KEY = "wb.detach.v1""#)
                && link.contains(r#"const CHANNEL = "wb.detach.v1""#),
            "wb-detach-link.js must keep the one registry key and channel name (#347)"
        );
        // The boot-ordering INVARIANT: a fence detached before the reload must
        // never have its members put back on the plane, for ANY verdict —
        // `relaunch` would spawn a SECOND PTY against a console the popup drives.
        assert!(
            body("function restoreDesk(").contains("record && away.has(record.id)"),
            "restoreDesk must skip a detached fence's members (#347)"
        );
        // `reconcileDesk` emits `record: null` on the `adopt` verdict, and the
        // skip runs BEFORE the dispatch — an unguarded read threw into the
        // swallowing `.catch`, costing every fence and every glyph on any boot
        // carrying a live session no record claims.
        assert!(
            js.contains(r#"out.push({ record: null, session: s, action: "adopt" })"#),
            "the adopt verdict's null record is what the guard above exists for (#347)"
        );
        // The registry carries the MEMBER IDS, not just the fence ids: a
        // detached fence may still be moved on the plane (§7a), after which a
        // geometry-derived membership answers "no members" and every one of
        // them comes back under a live popup.
        assert!(
            link.contains("readMembers()") && js.contains("link.readMembers()"),
            "the registry must carry each detached fence's member ids (#347)"
        );
        // The registry mirror and the store change in ONE place, so no return
        // path can leave them disagreeing.
        assert!(
            body("function commitDetached(")
                .contains("link.writeRegistry(detached, detachedMembers())"),
            "every detach transition must go through commitDetached (#347)"
        );
        assert!(
            !js.contains("detached = out.registry"),
            "no caller may assign the registry behind commitDetached's back (#347)"
        );
        // THE F5 RULE: `pagehide` fires on a reload exactly as on a close, so it
        // must no longer close a popup. The `putSync` pin is the NEGATIVE
        // CONTROL that the listener itself was not simply deleted.
        assert!(
            !body("window.addEventListener(\"pagehide\"").contains(".handle.close()"),
            "pagehide must not close a detached popup — that is the F5 (#347)"
        );
        assert!(
            js.contains("deskSink.putSync("),
            "the pagehide flush must survive the popup-close deletion (#347)"
        );
        // THE POPUP'S FIFTH DENIED CAPABILITY and its half of the lifecycle.
        for pin in [
            "detachLink: window.WBDetachLink.none()",
            // The channel-only factory: this document must reach no store, not
            // even to read the copy `window.open` handed it.
            "window.WBDetachLink.channel()",
            "\"popup-here\"",
            "\"popup-gone\"",
            // The BEHAVIOUR, not the class name: `detached-lost` alone is
            // satisfied by the CSS rule in this file's own <style> block, so
            // deleting `lost()` outright would keep a bare-noun pin green.
            r#"'<p class="detached-lost">"#,
            "window.close()",
            "WBConsole.peerFold(",
        ] {
            assert!(
                html.contains(pin),
                "detached-fence.html must keep the #347 pin {pin}"
            );
        }
        // #346's confidentiality control is UNCHANGED: the channel carries only
        // lifecycle chatter, the members still ride the concrete-origin handshake.
        assert!(
            html.contains(": window.location.origin;"),
            "the initial handover must keep its concrete targetOrigin (#346, #347)"
        );
        assert!(
            !html.contains("localStorage") && !html.contains("sessionStorage"),
            "the popup must still store nothing in the browser (#346)"
        );
        // SCRIPT ORDER, the same hazard #339/#346 pinned: `wb-console.js`
        // hard-dereferences `window.WBDetachLink` at module load, so a reordered
        // or dropped tag throws out of the whole IIFE while every pin above
        // stays green.
        for (doc, name) in [(shell, "index.html"), (html, "detached-fence.html")] {
            let link_tag = doc
                .find(r#"<script src="wb-detach-link.js"></script>"#)
                .unwrap_or_else(|| panic!("{name} must load wb-detach-link.js (#347)"));
            let console_tag = doc
                .find(r#"<script src="wb-console.js"></script>"#)
                .unwrap_or_else(|| panic!("{name} must load wb-console.js (#347)"));
            assert!(
                link_tag < console_tag,
                "{name}: wb-detach-link.js must be script-tagged BEFORE wb-console.js (#347)"
            );
        }
    }

    /// The stage/viewport shell (#336). Neither `node --test` nor Playwright
    /// runs in CI, so this is the only CI-visible gate that the deletion stays
    /// deleted — a re-added clamp would pass every unit test in the tree.
    #[test]
    fn shell_has_no_clamp_and_carries_the_stage() {
        let js = include_str!("../assets/ui/wb-console.js");
        assert!(
            !js.contains("clampAll"),
            "the clamp-and-refit is deleted, not renamed (#336)"
        );
        assert!(
            !js.contains("observeWorkspace"),
            "nothing observes the viewport to reposition a window (#336)"
        );
        // The NEGATIVE control for the two pins above: the per-window fit
        // observer must SURVIVE. It resizes a terminal, never a window rect, so
        // a blanket "no ResizeObserver" edit would be the wrong fix.
        assert!(
            js.contains("new ResizeObserver"),
            "the per-window terminal fit observer must survive the deletion (#336)"
        );
        assert!(
            js.contains("function stageExtent("),
            "the stage extent is a pure function in the shell (#336)"
        );

        let html = include_str!("../assets/ui/index.html");
        assert!(
            html.contains(r#"id="stage""#),
            "index.html must carry the stage plane (#336)"
        );
        assert!(
            !html.contains(r#"class="stage""#),
            "the old tab-body class is renamed .tabbody — one meaning per name (#336)"
        );

        let css = include_str!("../assets/ui/styles.css");
        for pin in ["#stage {", "#workspace.maxlock {", ".tabbody {"] {
            assert!(css.contains(pin), "styles.css must keep the #336 pin {pin}");
        }
    }

    /// The navigation layer over that plane (#337). Same reason as above: the
    /// node table and the Playwright pass both run out of CI, so this is the
    /// only gate that a gesture deleted here is a red test rather than a
    /// silently unreachable window.
    #[test]
    fn shell_navigates_the_plane() {
        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function bringIntoView(",
            "function panNudge(",
            "function reveal(",
            "function onFloorDown(",
            "function onWheel(",
            // The REGISTRATION, not the phrase: pinning a bare `passive: false`
            // is satisfied by the comment that explains it, so the option could
            // be deleted with this gate still green.
            r#"addEventListener("wheel", onWheel, { passive: false })"#,
            // the auto-pan loop's teardown — an uncancelled rAF pans forever
            // after the button is released
            "cancelAnimationFrame",
            // …and its two lost-mouseup recoveries, which are the only reason
            // that teardown is reachable when the release never arrives
            "ev.buttons === 0",
            r#"window.addEventListener("blur", onUp)"#,
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #337 pin {pin}"
            );
        }
        assert!(
            !js.contains("scrollIntoView"),
            "the centring is a tabled pure function, not the browser's heuristic (#337)"
        );

        let css = include_str!("../assets/ui/styles.css");
        for pin in [
            // The VALUE, not the property: `overscroll-behavior: auto` is the
            // exact mutation `wb_pan_337.py` measures as chaining the wheel out
            // of the terminal (plane 200 -> 0), and a property-name pin passes
            // straight through it.
            "overscroll-behavior: contain",
            "#stage.panning {",
            "cursor: grabbing",
        ] {
            assert!(css.contains(pin), "styles.css must keep the #337 pin {pin}");
        }
        // `cursor: grab` alone is satisfied by `.session-titlebar`, so scope the
        // pin to the stage's OWN rule — the floor advertising itself as the pan
        // surface is the affordance this issue added.
        let stage_rule = css
            .split_once("\n#stage {")
            .expect("styles.css must keep the #stage rule")
            .1;
        assert!(
            stage_rule[..stage_rule.find('}').expect("the #stage rule must close")]
                .contains("cursor: grab"),
            "the stage itself must advertise the grab cursor (#337)"
        );

        // The picker's wiring lives in app.js; without this the `@click`
        // handlers in index.html can go dangling with every test still green.
        let app = include_str!("../assets/ui/app.js");
        for pin in ["toggleWindowMenu(", "revealWindow(", "windowList"] {
            assert!(app.contains(pin), "app.js must keep the #337 pin {pin}");
        }

        let html = include_str!("../assets/ui/index.html");
        for pin in [r#"class="dropdown window-menu""#, r#"class="window-item""#] {
            assert!(
                html.contains(pin),
                "index.html must keep the #337 pin {pin}"
            );
        }
    }

    /// The FRAME chrome over that plane (#338): the footer pills and the
    /// empty-stage hint belong to `.consoles-tab`, never to `#stage`, and the
    /// maximize pin is a DERIVED fact. Same bargain as the two above — neither
    /// `node --test` nor Playwright runs in CI, so this is the only gate that
    /// notices the chrome sliding back onto the plane.
    #[test]
    fn shell_pins_the_frame_chrome() {
        let html = include_str!("../assets/ui/index.html");
        // The stage element is EMPTY in the markup, so the chrome physically
        // cannot be a child of it — a stronger pin than "is not inside" prose.
        assert!(
            html.contains(r#"<div id="stage"></div>"#),
            "the stage must stay an empty element the shell fills (#338)"
        );
        let tab = html
            .find(r#"class="consoles-tab""#)
            .expect("index.html must keep the consoles tab (#338)");
        // The section's OWN close, not a later landmark: bounding by `#viewers`
        // would still pass for chrome that escaped the tab entirely.
        let close = tab
            + html[tab..]
                .find("</section>")
                .expect("the consoles tab must close (#338)");
        // The viewport as a CLOSED element: an offset past its `</div>` cannot be
        // inside `#workspace` — and therefore cannot be inside `#stage` either.
        // Chrome inside the scrolling box pans away just as surely as chrome on
        // the plane, so "after the stage" alone would not be enough.
        let viewport = r#"<div id="workspace"><div id="stage"></div></div>"#;
        let ws = html
            .find(viewport)
            .expect("index.html must keep the closed viewport element (#338)")
            + viewport.len();
        assert!(
            ws > tab,
            "the viewport must live inside the consoles tab (#338)"
        );
        // `canvas-empty` was the second piece of frame chrome here. The
        // empty-stage caption is gone — the toolbar's New-console button says
        // the same thing where the operator acts — so the footer carries the
        // rule alone, and the caption's absence is asserted below.
        let pin = r#"class="canvas-foot""#;
        let at = html
            .find(pin)
            .unwrap_or_else(|| panic!("index.html must carry the frame chrome {pin} (#338)"));
        assert!(
            at > ws && at < close,
            "{pin} must be a SIBLING of the viewport inside .consoles-tab (#338)"
        );
        // The ELEMENT, not the words: the comment that stands where the caption
        // did quotes them, and a pin that cannot tell prose from markup would
        // make documenting the removal the thing that fails.
        assert!(
            !html.contains(r#"class="canvas-empty""#),
            "the empty-stage caption is not to come back"
        );

        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function syncMaxPin(",
            // The REGISTRATION, not the function: without it the pin is only
            // re-derived on a maximize and a programmatic pan desyncs it.
            r#"ws.addEventListener("scroll", syncMaxPin)"#,
            "workbench:stage-extent",
            // The NEGATIVE control: the scroll freeze must SURVIVE. A blanket
            // deletion of the maximize machinery would satisfy every "no longer
            // contains" pin below and must be red, not green.
            "function syncMaxLock(",
            // The POSITIVE half of the `reveal()` change: the negative pin below
            // is one spelling and a requote would slip past it, and scenario 4
            // (the only behavioural gate) does not run in CI.
            r#"if (it.classList.contains("maximized")) return it;"#,
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #338 pin {pin}"
            );
        }
        assert!(
            !js.contains(r#"if (ws.classList.contains("maxlock")) return it;"#),
            "reveal() must pan the plane while maximized — Go-to is the path (#338)"
        );

        let css = include_str!("../assets/ui/styles.css");
        let foot = css
            .split_once("\n.canvas-foot {")
            .expect("styles.css must keep the .canvas-foot rule (#338)")
            .1;
        let foot = &foot[..foot.find('}').expect("the .canvas-foot rule must close")];
        for pin in ["pointer-events: none", "z-index: 130"] {
            assert!(
                foot.contains(pin),
                "the .canvas-foot rule must keep {pin} — above the consoles, inert to the pointer (#338)"
            );
        }
    }

    /// The per-client view (#339) and — the part that can silently regress — the
    /// ONE place allowed to name `localStorage`. ADR-0050 §3 dropped the browser
    /// desk store; ADR-0051 §8 narrows that to "no *desk* in browser storage",
    /// which is only honest while the view store stays a single module holding a
    /// single key. Same bargain as the pins above: neither `node --test` nor
    /// Playwright runs in CI, so this is the gate that notices the desk creeping
    /// back into the browser.
    #[test]
    fn shell_stores_only_the_view_in_the_browser() {
        // The two modules that own the state being persisted must reach it only
        // through `WBView` — a direct write from either is how a second store
        // starts.
        for (name, src) in [
            ("wb-console.js", include_str!("../assets/ui/wb-console.js")),
            ("app.js", include_str!("../assets/ui/app.js")),
        ] {
            assert!(
                !src.contains("localStorage"),
                "{name} must not touch localStorage — wb-view.js owns the store (#339)"
            );
        }

        let view = include_str!("../assets/ui/wb-view.js");
        // NEGATIVE CONTROL: deleting the store wholesale would satisfy every
        // "does not contain" assertion above. It must be red, not green.
        assert!(
            view.contains("localStorage"),
            "wb-view.js IS the browser store — deleting it is not how #339 stays green"
        );
        assert!(
            view.contains(r#"const KEY = "wb.view.v1""#),
            "wb-view.js must keep the one view key (#339)"
        );

        let js = include_str!("../assets/ui/wb-console.js");
        for pin in [
            "function viewLanding(",
            "function applyLanding(",
            // The REGISTRATION, not the function: without it the offset is never
            // persisted and the landing has nothing to restore.
            r#"ws.addEventListener("scroll", saveOffset)"#,
        ] {
            assert!(
                js.contains(pin),
                "wb-console.js must keep the #339 pin {pin}"
            );
        }

        // Tree-wide, not just the two modules above: any non-vendor asset that
        // starts naming `localStorage` is a second store by definition. `.html`
        // is swept too — both shells carry inline `<script>` blocks, so a store
        // could grow there without touching a single `.js`.
        for path in embedded_ui_paths() {
            let scanned = path.ends_with(".js") || path.ends_with(".html");
            if !scanned || path.starts_with("vendor/") || path == "wb-view.js" {
                continue;
            }
            let src = UI
                .get_file(&path)
                .and_then(|f| f.contents_utf8())
                .unwrap_or_else(|| panic!("{path} must be embedded as UTF-8"));
            assert!(
                !src.contains("localStorage"),
                "{path} must not touch localStorage — wb-view.js is the only store (#339)"
            );
        }

        // The store must be defined BEFORE its readers run: `wb-console.js`
        // reads `WBView` on its boot path, and a later tag would leave the
        // landing reading `undefined` on the very first paint.
        let html = include_str!("../assets/ui/index.html");
        let view_tag = html
            .find(r#"<script src="wb-view.js"></script>"#)
            .expect("index.html must load wb-view.js (#339)");
        let console_tag = html
            .find(r#"<script src="wb-console.js"></script>"#)
            .expect("index.html must load wb-console.js (#339)");
        assert!(
            view_tag < console_tag,
            "wb-view.js must be script-tagged BEFORE wb-console.js (#339)"
        );
    }

    /// Silence is not death. A browser throttles a hidden tab's timers to one
    /// tick per minute after about five minutes, so the six-second heartbeat
    /// window closed a working popup and pulled its consoles home mid-work. Both
    /// documents now demand a better witness before acting — a window handle,
    /// or a probe that message delivery (which is not timer-throttled) still
    /// answers. `wb_fence_347.py` scenario 5c drives it; this is CI's view.
    #[test]
    fn a_quiet_detach_peer_is_challenged_before_it_is_buried() {
        let js = include_str!("../assets/ui/wb-console.js");
        assert!(
            js.contains("function stillThere(id, entry) {"),
            "the origin must ask whether a quiet popup is really gone"
        );
        assert!(
            js.contains("&& !stillThere(id, entry)"),
            "the re-attach must be gated on that answer, not on the fold alone"
        );
        assert!(
            js.contains(r#"m.type === "popup-ping""#) && js.contains(r#"type: "origin-here""#),
            "the origin must answer a probe from its MESSAGE handler, not a timer"
        );
        let popup = include_str!("../assets/ui/detached-fence.html");
        assert!(
            popup.contains("!window.opener || window.opener.closed"),
            "the popup's verdict is the opener HANDLE, which owes nothing to a timer"
        );
        assert!(
            popup.contains("if (out.effects.some((x) => x.type === \"peer-lost\")) silent();"),
            "a lost peer must reach `silent`, which probes, and never `lost` directly"
        );
    }

    /// Three pieces of chrome that only a browser can really prove, pinned here
    /// because neither `node --test` nor Playwright runs in CI.
    #[test]
    fn the_console_chrome_holds_its_three_rules() {
        let app = include_str!("../assets/ui/app.js");
        let html = include_str!("../assets/ui/index.html");
        // ONE dropdown at a time. Every toggler goes through `closeMenus`, which
        // enumerates the four in ONE place — the account menu and the toolbar's
        // pickers used to enumerate each other and left both open, overlapping.
        assert!(
            app.contains("closeMenus() {") && app.contains("this.avatarMenu = false;"),
            "app.js must close every menu from one place"
        );
        for pin in ["toggleAvatarMenu()", "toggleAgentMenu()"] {
            assert!(html.contains(pin), "index.html must toggle through {pin}");
        }
        assert!(
            !html.contains("avatarMenu = !avatarMenu"),
            "the account button must not toggle its own flag past the others"
        );
        // One picture for one thing: the Consoles tab, the New-console button and
        // the rows in its menu all wear the same terminal glyph.
        assert!(
            app.contains(r#"icon: "bi bi-terminal""#) && !app.contains("bi-robot"),
            "the Consoles tab must wear the terminal glyph, not a robot"
        );
        // A fence name is read-only until asked for twice: its title bar is also
        // what the operator clicks to reach the fence, and an always-live input
        // turned every such slip into a rename.
        let js = include_str!("../assets/ui/wb-console.js");
        assert!(
            js.contains("name.readOnly = true;")
                && js.contains(r#"name.addEventListener("dblclick""#),
            "the fence name must open on a double click and close on blur"
        );
    }

    /// The three clicks that ask first. Tiling a fence moves every console in
    /// it, removing a fence takes the region out from under them, and a
    /// console's × ends a live session — all one pixel from something harmless
    /// on the same title bar. The dialog is built in `wb-console.js` rather
    /// than borrowed from the shell's Alpine one, because the module also runs
    /// in the detached-fence popup, which has neither; `window.confirm` is
    /// pinned OUT because an automated browser dismisses it by default, which
    /// would turn every guarded click into a silently cancelled one.
    #[test]
    fn the_destructive_console_clicks_confirm_first() {
        let js = include_str!("../assets/ui/wb-console.js");
        assert!(
            js.contains("function askConfirm({"),
            "wb-console.js must own a confirmation dialog of its own"
        );
        assert!(
            !js.contains("window.confirm("),
            "the native confirm is not the seam — an automated browser dismisses it"
        );
        // The three call sites, each awaiting the answer before acting.
        for pin in [
            r#"title: "Tile this fence?""#,
            r#"title: "Remove this fence?""#,
            r#"title: "Close this console?""#,
        ] {
            assert!(js.contains(pin), "wb-console.js must keep the pin {pin}");
        }
        // The EXPORTED verbs stay unguarded: a caller that names `arrangeFence`
        // has already decided, and the dialog belongs to the accidental click.
        let verb = js
            .split_once("\n  function removeFence(id) {")
            .expect("wb-console.js must keep removeFence")
            .1;
        assert!(
            !verb[..verb.find("\n  }").expect("removeFence must close")].contains("askConfirm"),
            "the verb must not ask — only the button does"
        );
        // The dialog wears the shell's own modal classes, so the popup (which
        // loads the same stylesheet and no Alpine) shows the same dialog.
        assert!(
            js.contains(r#"scrim.className = "modal-scrim wb-confirm""#),
            "the dialog must reuse the shared modal chrome"
        );
    }

    /// The standing authorization to relaunch agent consoles on load. It is a
    /// spending decision — one vendor CLI per saved agent console, on every page
    /// load — so what this pins is that it stays OFF unless the operator turned
    /// it on, and that it stays in the BROWSER: daemon-wide, every tab pointed
    /// at the same desk would relaunch the same consoles and spend the quota
    /// once per tab. Playwright proves the fold (`wb_desk_303.py` scenario 2);
    /// this is the gate CI can see.
    #[test]
    fn relaunching_agent_consoles_on_load_is_opt_in() {
        let js = include_str!("../assets/ui/wb-console.js");
        assert!(
            js.contains(
                "record.kind === \"console\" || relaunchAgents ? \"relaunch\" : \"placeholder\""
            ),
            "the restore fold must relaunch an agent console ONLY under the opt-in"
        );
        // The DEFAULT is the whole guard: a caller that omits the option — the
        // fold's own tests, a later call site — must get the parked placeholder.
        assert!(
            js.contains("relaunchAgents = false }"),
            "an omitted `relaunchAgents` must default to false, never to launching"
        );
        // The relaunch verdict must ask for the record's OWN kind. `{ console:
        // true }` is the shell request, and reaching an agent record with it —
        // which the opt-in made possible — opened a plain shell in the agent's
        // box (measured in `wb_desk_303.py` scenario 10 before this line).
        assert!(
            js.contains(
                r#"record.kind === "agent" ? { repo, agent: record.agent } : { console: true, repo }"#
            ),
            "a relaunched agent console must be requested by its vendor, not as a shell"
        );
        // The popup holds a fragment of the plane and authors no session; its
        // injected `viewStore` reads nothing, and `canLaunch` refuses besides.
        assert!(
            js.contains("OPTS.canLaunch !== false && viewStore?.read()?.relaunch === true"),
            "the opt-in must be read through the injected view store, and never in the popup"
        );

        // The knob, and the store it writes to. `config.set` would put a
        // per-browser choice in a repo's settings.json for every client to obey.
        let settings = include_str!("../assets/ui/wb-settings.js");
        for pin in [
            r#"scope: "client""#,
            r#"key: "consoles.relaunch_on_load""#,
            r#"type: "toggle""#,
        ] {
            assert!(
                settings.contains(pin),
                "wb-settings.js must keep the pin {pin}"
            );
        }
        let app = include_str!("../assets/ui/app.js");
        assert!(
            app.contains("window.WBView.patch({ relaunch: value === true })"),
            "app.js must persist the toggle through the view store"
        );
        assert!(
            app.contains("if (this.CLIENT_KEYS.has(key)) {"),
            "a client-scoped key must return before the config.set path"
        );
        let html = include_str!("../assets/ui/index.html");
        assert!(
            html.contains(r#"it.type === 'toggle'"#),
            "index.html must render the toggle control"
        );
    }

    /// The plan viewer's prose is keyed to the issue the plan says it is for.
    /// Same CI bargain as the pins below: `node --test` covers the helpers and
    /// Playwright covers the rendering, and CI runs neither.
    ///
    /// The defect this guards: the steps come from the run snapshot and are keyed
    /// by issue (ADR-0047 A1), but the prose is a `file.read` of `.ralphy/plan.md`
    /// — which holds the PREVIOUS issue's plan for the whole planning phase of the
    /// next one. Without the key the block renders that plan as the current one.
    #[test]
    fn the_plan_prose_is_keyed_to_the_issue_the_plan_names() {
        let runs_js = include_str!("../assets/ui/wb-runs.js");
        // The literal, cross-checked against its PRODUCER: the planner writes
        // `plan_trailer` (crates/ralphy-adapter-support/src/resume.rs). The daemon
        // does not depend on that crate (leaf-crate rule, ADR-0032 §10), so the
        // shared shape is pinned by literal here and named there.
        assert!(
            runs_js.contains("ralphy-plan:") && runs_js.contains("issue="),
            "wb-runs.js must read the plan trailer written by resume.rs `plan_trailer`"
        );
        for pin in ["planTrailerIssue(", "planBelongsTo("] {
            assert!(
                runs_js.contains(pin),
                "wb-runs.js must keep the helper {pin}"
            );
        }
        let app_js = include_str!("../assets/ui/app.js");
        let squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        // Both readers of the prose go through the SAME gate — a picker that
        // offered a stale plan's headings would be the identical lie one level up.
        assert!(
            squeezed.contains("planHeadings(run) { if (!this.planProseIsCurrent(run)) return [];"),
            "planHeadings must withhold a stale plan's sections"
        );
        assert!(
            squeezed.contains("if (!run || !name || !this.planProseIsCurrent(run)) return \"\";"),
            "renderPlanSection must refuse prose that belongs to another issue"
        );
    }

    /// Stopping a run — the one control in the Runs panel that throws away work
    /// in progress (ADR-0054, ADR-0032 §6) — asks through the shell's own dialog.
    /// `window.confirm` names the origin, ignores the theme and blocks the page,
    /// which is the wrong furniture for the panel's most consequential click.
    #[test]
    fn stopping_a_run_confirms_through_the_design_system_dialog() {
        let app_js = include_str!("../assets/ui/app.js");
        let squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squeezed.contains(r#"const ok = await this.askConfirm({ title: "Stop this run?","#),
            "stopRun must confirm through askConfirm, not window.confirm"
        );
        // The remaining native calls are the DOCUMENTED fallback for an
        // unreachable shell (`getShell()` returning null), so they are counted
        // rather than forbidden: the count is what reds if a new one appears.
        assert_eq!(
            app_js.matches("window.confirm(").count(),
            1,
            "the only window.confirm left is the shell-unreachable fallback"
        );
    }

    /// The board can see, read and throw away the plan that the NEXT RUN will
    /// execute. Same CI bargain as the pins around it; the structural half of the
    /// slice, since neither Playwright nor `node --test` runs in CI.
    #[test]
    fn the_board_surfaces_the_plan_the_next_run_would_execute() {
        let runs_js = include_str!("../assets/ui/wb-runs.js");
        for pin in [
            "planSummary(",
            "planPillLabel(",
            "planPillWarns(",
            "isBundleReason(",
        ] {
            assert!(runs_js.contains(pin), "wb-runs.js must keep {pin}");
        }
        // The verdict must be the RUNNER's test — zero open steps
        // (ralphy-core `plan::count_open_steps`, read by runner/phases.rs) — and
        // never the `## Feasible:` heading's claim, which is the human's reason.
        // A heading-driven verdict would call a plan with nothing to do "ready".
        let squeezed: String = runs_js.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squeezed.contains("infeasible: openSteps === 0,"),
            "infeasible must mean zero OPEN STEPS, mirroring count_open_steps"
        );

        let app_js = include_str!("../assets/ui/app.js");
        for pin in [
            r#"path: ".ralphy/plan.md","#,
            r#"window.WBDaemon.write("plan.discard", { repo: slug })"#,
        ] {
            assert!(app_js.contains(pin), "app.js must keep {pin}");
        }
        // The discard is confirmed, and the plan is only ever shown against the
        // issue its trailer names (`planFor`) — a plan offered on the wrong card
        // would invite a discard of the wrong work.
        let app_squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            app_squeezed
                .contains(r#"const ok = await this.askConfirm({ title: "Discard this plan?","#),
            "discardPlan must confirm first"
        );
        assert!(
            app_squeezed.contains("return held && held.summary.issue === number ? held : null;"),
            "a plan is shown against the issue it names, and no other"
        );

        let html = include_str!("../assets/ui/index.html");
        for pin in [
            r#"class="kc-plan""#,
            r#"class="kanban-plan-chip""#,
            r#"class="modal plan-modal""#,
            r#"data-act="plan-discard""#,
        ] {
            assert!(html.contains(pin), "index.html must keep {pin}");
        }
        // Gated while a run holds the repo: a run owns the plan it is executing.
        let discard = html
            .find(r#"data-act="plan-discard""#)
            .expect("the discard control must exist");
        assert!(
            html[discard..discard + 220].contains(":disabled=\"writeLocked()\""),
            "the discard must be gated while a run holds the repo"
        );
        // A dialog a MODAL asks for must sit above it. Every `.modal-scrim` shares
        // `z-index: 500`, so the winner was DOM order — the plan modal's scrim
        // covered the confirm it had just raised, leaving Escape as the only
        // reachable control. Measured with Playwright, which named the plan scrim
        // as the interceptor; the fix raises the ASKED-FOR dialog rather than
        // reordering the markup, so it cannot regress by a paste in the wrong spot.
        let css = include_str!("../assets/ui/styles.css");
        assert!(
            css.contains(".modal-scrim:has(> .confirm-modal),")
                && css.contains(".modal-scrim:has(> .prompt-modal)"),
            "the confirm/prompt dialogs must outrank the modal that raised them"
        );
    }

    /// The plan blocks' chrome: the note explains the list from ABOVE it, and the
    /// section picker looks like the dropdown it is. Structural, not cosmetic —
    /// both defects are invisible to every other test in this file.
    #[test]
    fn the_plan_blocks_explain_themselves_before_the_space_they_describe() {
        let html = include_str!("../assets/ui/index.html");
        let note = html
            .find(r#"class="plan-steps-note""#)
            .expect("index.html must keep the steps note");
        let list = html
            .find(r#"<ul class="plan-steps">"#)
            .expect("index.html must keep the steps list");
        assert!(
            note < list,
            "the steps note must precede the list it explains — under an empty \
             list it sits at the bottom of the block, away from the space it is about"
        );
        // `all: unset` on `.plan-picker` removes the native arrow, so the caret is
        // the only thing saying the head opens. Its inertness is the other half:
        // a caret that swallows the click advertises an act it then prevents.
        assert!(
            html.contains(r#"class="plan-picker-caret" data-lucide="chevron-down""#),
            "the section picker must carry a caret (`all: unset` drops the native one)"
        );
        let css = include_str!("../assets/ui/styles.css");
        let caret = css
            .find(".plan-picker-caret {")
            .expect("styles.css must style the picker caret");
        assert!(
            css[caret..caret + 200].contains("pointer-events: none"),
            "the caret must be inert to the pointer so the click reaches the select"
        );
    }

    /// The runs panel's chrome (#331). Neither `node --test` nor Playwright runs
    /// in CI, so these substrings are the only CI-visible gate over the markup —
    /// the same bargain #318/#319 struck for the write controls.
    #[test]
    fn the_runs_feed_is_contained_in_the_markup() {
        let html = include_str!("../assets/ui/index.html");
        for pin in [
            r#"class="runs-feed""#,
            r#"data-act="feed-collapse""#,
            r#"data-act="feed-dismiss""#,
            r#"x-show="rawFeedOpen""#,
            r#"class="runs-verb-error""#,
            "verbLocked()",
            "verbTitle('triage')",
        ] {
            assert!(
                html.contains(pin),
                "index.html must keep the #331 pin {pin}"
            );
        }
        // The NEGATED pin, written STRUCTURALLY rather than as one spelling of
        // the old tag: `<pre x-show="rawFeed" class="runs-raw">` is the same
        // defect with the attributes swapped. The invariant is that the feed
        // occurs exactly once and is INSIDE the sized box, so the box's class
        // must appear before it.
        assert_eq!(
            html.matches("runs-raw").count(),
            1,
            "the raw feed must occur exactly once in index.html (#331)"
        );
        let feed_box = html
            .find(r#"class="runs-feed""#)
            .expect("index.html must keep the .runs-feed box");
        let raw = html
            .find("runs-raw")
            .expect("index.html must keep the raw feed");
        assert!(
            feed_box < raw,
            "the raw feed must stay INSIDE its sized .runs-feed box (#331)"
        );

        // The toolbar states the run lock in each disabled control's `title` and
        // NOWHERE else: the visible note was removed on the operator's own
        // request, so its absence is the pin. Negated, because a reflex to
        // "explain the dimmed button on screen" is exactly what would put it back.
        assert!(
            !html.contains("runs-lock-note"),
            "the runs toolbar carries no standing lock message — the reason rides \
             each disabled verb's title (`verbTitle`)"
        );

        let app_js = include_str!("../assets/ui/app.js");
        // `rawFeedOpen: false` is the DEFAULT, not an incidental initialiser: the
        // feed can take 30vh of a panel whose job is the trail and the plan, so
        // the bytes are opt-in and only the head arrives with the output.
        for pin in ["dismissFeed()", "runVerbFailed(", "rawFeedOpen: false"] {
            assert!(app_js.contains(pin), "app.js must keep the #331 pin {pin}");
        }
        assert!(
            !app_js.contains("rawFeedOpen = true"),
            "no reset may re-open the feed: dismiss and every verb click return it \
             to the collapsed default"
        );
        // The gate REUSES the Changes derivation rather than paralleling it —
        // that reuse is the acceptance criterion, so it is pinned. Whitespace
        // is collapsed first so the pin judges the CODE and not its layout: it
        // must survive a reformat and a CRLF checkout (this host holds LF in
        // the blob and CRLF on disk) without blaming a design rule for either.
        let squeezed: String = app_js.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squeezed.contains("verbLocked() { return this.writeLocked(); }"),
            "verbLocked() must be literally writeLocked(), not a second predicate (#331)"
        );

        let runs_js = include_str!("../assets/ui/wb-runs.js");
        for pin in ["verbLockTitle(", "exitNote("] {
            assert!(
                runs_js.contains(pin),
                "wb-runs.js must keep the #331 helper {pin}"
            );
        }
        assert!(
            include_str!("../assets/ui/wb-daemon.js").contains("runVerbFailed?.("),
            "wb-daemon.js must route a terminal verb frame to the panel (#331)"
        );
    }

    /// The runs chrome's own colour gate — plus the declarations that actually
    /// DO the bounding. The markup pins above prove the box exists; only these
    /// prove it is bounded, and `max-height` is a single line whose deletion
    /// restores the original defect with every other pin still green.
    #[test]
    fn the_runs_chrome_adds_no_colour_outside_the_token_set() {
        let css = include_str!("../assets/ui/styles.css");
        let open = "/* #331 runs chrome */";
        let close = "/* #331 runs chrome end */";
        let start = css
            .find(open)
            .expect("styles.css must keep the #331 runs-chrome opening marker")
            + open.len();
        let end = css
            .find(close)
            .expect("styles.css must keep the #331 runs-chrome closing marker");
        let block = &css[start..end];
        // Every assertion below is a `contains`, so an emptied block would
        // satisfy only the negative one — check it has content first.
        assert!(
            !block.trim().is_empty(),
            "the #331 runs-chrome block must not be empty"
        );
        assert!(
            !block.contains('#'),
            "the #331 runs-chrome CSS must reference var(--…) tokens only, no hex literals"
        );
        // The bound, the wrap, and the containment: the three declarations the
        // issue's criteria rest on. The browser pass measures them, but neither
        // `node --test` nor Playwright runs in CI (lib.rs doc above).
        for decl in [
            "max-height: 30vh",
            "overflow-wrap: anywhere",
            "white-space: pre-wrap",
            "flex: 0 0 auto",
        ] {
            assert!(
                block.contains(decl),
                "the feed's containment rests on `{decl}` — it must stay in the #331 block"
            );
        }
        // The phone width is a criterion, so the narrow rule is pinned by its
        // BREAKPOINT and its payload: a bare `@media` would be satisfied by
        // `@media print {}` while the narrow cap was deleted.
        let narrow = block
            .find("@media (max-width: 560px)")
            .expect("the phone width is a criterion — keep the (max-width: 560px) rule");
        assert!(
            block[narrow..].contains("max-height: 22vh"),
            "the narrow-width rule must still cap the feed"
        );
    }

    /// The discard block's own colour + hover gate, reusing #318's scan. It also
    /// asserts the block still holds an `@media` rule: the touch de-emphasis IS
    /// the criterion, and a block that lost it would pass the rest vacuously.
    #[test]
    fn the_discard_controls_add_no_colour_outside_the_token_set() {
        let css = include_str!("../assets/ui/styles.css");
        let open = "/* #319 discard */";
        let close = "/* #319 discard end */";
        let start = css
            .find(open)
            .expect("styles.css must keep the #319 discard opening marker")
            + open.len();
        let end = css
            .find(close)
            .expect("styles.css must keep the #319 discard closing marker");
        let block = &css[start..end];
        assert!(
            !block.contains('#'),
            "the #319 discard CSS must reference var(--…) tokens only, no hex literals"
        );
        assert!(
            block.contains("@media"),
            "the touch de-emphasis is the criterion — the block must keep its @media rule"
        );

        let declarations = strip_css_comments(block);
        let mut hover_rules = 0;
        for rule in declarations.split('}') {
            let Some((selector, body)) = rule.split_once('{') else {
                continue;
            };
            if !selector.contains(":hover") {
                continue;
            }
            hover_rules += 1;
            for banned in ["opacity", "visibility", "display", "max-height"] {
                assert!(
                    !body.contains(banned),
                    "a discard control must not be hover-gated on {banned}: {selector}"
                );
            }
        }
        assert!(
            hover_rules >= 2,
            "the discard block must still carry its :hover rules, found {hover_rules}"
        );
    }

    /// CSS text with every `/* … */` comment removed, so a rule scan judges
    /// selectors and not the prose that names one as prior art.
    fn strip_css_comments(block: &str) -> String {
        let mut out = String::new();
        let mut rest = block;
        while let Some(at) = rest.find("/*") {
            out.push_str(&rest[..at]);
            match rest[at + 2..].find("*/") {
                Some(end_at) => rest = &rest[at + 2 + end_at + 2..],
                None => {
                    rest = "";
                    break;
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// The write controls' CSS must speak the shell's token language (ADR-0035)
    /// exactly as the rail view's does. Its own block, and its own marker pair:
    /// appending to #317's would silently widen a pin that names another issue.
    #[test]
    fn the_write_controls_add_no_colour_outside_the_token_set() {
        let css = include_str!("../assets/ui/styles.css");
        let open = "/* #318 write controls */";
        let close = "/* #318 write controls end */";
        let start = css
            .find(open)
            .expect("styles.css must keep the #318 write-controls opening marker")
            + open.len();
        let end = css
            .find(close)
            .expect("styles.css must keep the #318 write-controls closing marker");
        let block = &css[start..end];
        assert!(
            !block.contains('#'),
            "the #318 write-control CSS must reference var(--…) tokens only, no hex literals"
        );
        // The touch criterion, pinned where CI can see it: a hover-gated
        // `opacity`/`visibility` is exactly the affordance a phone cannot find.
        // Split on `}` so each chunk is one rule — selector, then its body — and
        // judge the BODY of any rule whose selector mentions `:hover`. Comments
        // are stripped FIRST: one of them names `.branch-chip.disabled:hover` as
        // prior art, and a raw split would read that prose as a selector.
        let declarations = strip_css_comments(block);

        let mut hover_rules = 0;
        for rule in declarations.split('}') {
            let Some((selector, body)) = rule.split_once('{') else {
                continue;
            };
            if !selector.contains(":hover") {
                continue;
            }
            hover_rules += 1;
            // `display` and `max-height` are in the list because the TEXTBOOK
            // hover-gated affordance is `display: none` + `:hover { display:
            // … }` — banning only `opacity`/`visibility` would leave the most
            // obvious spelling of the defect green.
            for banned in ["opacity", "visibility", "display", "max-height"] {
                assert!(
                    !body.contains(banned),
                    "a write control must not be hover-gated on {banned}: {selector}"
                );
            }
        }
        // …and the scan must have had something to judge: a block that stopped
        // carrying `:hover` rules would satisfy the loop above vacuously.
        assert!(
            hover_rules >= 2,
            "the write-control block must still carry its :hover rules, found {hover_rules}"
        );
    }

    /// The rail view's CSS must speak the shell's token language (ADR-0035), not
    /// invent colours: a hex literal anywhere in the block is the failure this
    /// catches. `cargo test` is the only gate CI runs over these assets.
    #[test]
    fn the_changes_view_adds_no_colour_outside_the_token_set() {
        let css = include_str!("../assets/ui/styles.css");
        let open = "/* #317 rail view */";
        let close = "/* #317 rail view end */";
        let start = css
            .find(open)
            .expect("styles.css must keep the #317 rail-view opening marker")
            + open.len();
        let end = css
            .find(close)
            .expect("styles.css must keep the #317 rail-view closing marker");
        let block = &css[start..end];
        assert!(
            !block.contains('#'),
            "the #317 rail-view CSS must reference var(--…) tokens only, no hex literals"
        );
    }

    /// The browser half of the run-completion nudge (#310) is exercised by
    /// `node --test` and a Playwright pass, neither of which CI runs — so the
    /// three symbols the push path hangs on are pinned from the Rust gate, the
    /// way #309 pinned the list's markup.
    #[test]
    fn the_run_completion_nudge_is_wired_through_the_ui_assets() {
        assert!(
            include_str!("../assets/ui/wb-changes.js").contains("function shouldReload("),
            "wb-changes.js must keep the shouldReload filter (#310)"
        );
        let daemon_js = include_str!("../assets/ui/wb-daemon.js");
        assert!(
            daemon_js.contains("function subscribeChanges("),
            "wb-daemon.js must keep the subscribeChanges socket (#310)"
        );
        assert!(
            daemon_js.contains("subscribeChanges,"),
            "wb-daemon.js must EXPORT subscribeChanges — app.js guards on it (#310)"
        );
        let app_js = include_str!("../assets/ui/app.js");
        for symbol in [
            "mountChangesSub()",
            "destroyChangesSub()",
            "shouldReload?.(",
        ] {
            assert!(
                app_js.contains(symbol),
                "app.js must keep {symbol} on the nudge path (#310)"
            );
        }
    }

    /// The tree's folder predicate must read Wunderbaum's `data` bag, never a
    /// bare `node.folder`. Wunderbaum copies source keys it does not itself
    /// define into `node.data`, so the `folder: true` the daemon-backed listing
    /// sets lands at `node.data.folder` and `node.folder` is always `undefined`
    /// — and `node.children` is `null` until a lazy folder expands. Reading
    /// either alone made EVERY collapsed folder answer "file", which silently
    /// took out five call sites at once: the context menu offered no create
    /// items, no subdirectory was ever added to the `/ws/tree` watch set,
    /// double-clicking a folder read it as bytes, `findFolderByRel` never
    /// resolved so subdirectory `tree.dirty` nudges were all dropped, and the
    /// reconcile lost descendant expansion. Only a browser sees that, and CI
    /// runs no browser — so the shape is pinned here.
    #[test]
    fn the_tree_folder_predicate_reads_wunderbaums_data_bag() {
        let js = include_str!("../assets/ui/app.js");
        let body = js
            .split_once("    isFolder(node) {")
            .expect("app.js no longer defines isFolder(node)")
            .1
            .split_once("\n    },")
            .expect("app.js's isFolder is never closed")
            .0;
        assert!(
            body.contains("node.data?.folder"),
            "isFolder must read node.data.folder (Wunderbaum's bag for unknown \
             source keys); found: {body:?}"
        );
        assert!(
            !body.contains("node.folder "),
            "isFolder must not read a bare node.folder — it is always undefined; \
             found: {body:?}"
        );
        assert!(
            body.contains("node.lazy"),
            "isFolder must accept a collapsed lazy folder, whose children are \
             still null; found: {body:?}"
        );
    }

    /// Creating must be reachable for every target the operator can point at:
    /// a folder, a file (meaning its parent), and the repo root. The root has
    /// no node — a right-click on empty tree space resolves to `null` — so the
    /// context handler must NOT bail on a missing node, or a top-level file is
    /// uncreatable. The Files header carries the same two actions, because
    /// right-clicking empty space is an affordance nothing on screen advertises.
    #[test]
    fn the_explorer_can_create_at_every_target_including_the_repo_root() {
        let js = include_str!("../assets/ui/app.js");
        assert!(
            js.contains("this.showMenu(ev.clientX, ev.clientY, node || null)"),
            "the tree's contextmenu handler must open the menu for a NULL node \
             (empty space = the repo root), not return early"
        );
        for symbol in [
            "emitCreate(node, kind) {",
            "createDir(node) {",
            "createHere(kind) {",
        ] {
            assert!(
                js.contains(symbol),
                "app.js must keep {symbol} — the create-target resolution"
            );
        }
        let html = include_str!("../assets/ui/index.html");
        for symbol in ["createHere('file')", "createHere('folder')"] {
            assert!(
                html.contains(symbol),
                "index.html's Files header must wire {symbol}"
            );
        }
    }

    /// Naming a new entry goes through the design-system prompt, not the
    /// browser's: `window.prompt` is unstyled, is suppressible for the whole
    /// origin by one "prevent this page from creating more dialogues" tick, and
    /// never renders in a detached popup. It stays only as the fallback for a
    /// shell that cannot be reached.
    #[test]
    fn naming_a_new_entry_uses_the_design_system_prompt() {
        let js = include_str!("../assets/ui/app.js");
        for symbol in [
            "askPrompt(opts = {}) {",
            "promptSubmit() {",
            "promptRespond(name) {",
        ] {
            assert!(js.contains(symbol), "app.js must keep {symbol}");
        }
        assert!(
            js.contains("await c.askPrompt({"),
            "the create path must ask for the name through askPrompt"
        );
        let html = include_str!("../assets/ui/index.html");
        for symbol in ["prompt-modal", "id=\"prompt-input\"", "promptSubmit()"] {
            assert!(
                html.contains(symbol),
                "index.html must render the prompt dialog ({symbol})"
            );
        }
    }

    /// A Changes row's action buttons must sit OUTSIDE the clipped region.
    ///
    /// `.chg-name` is frozen on purpose (`flex: 0 0 auto`, so an absurd name is
    /// never ellipsized while a directory can still drain), which means a long
    /// name genuinely overflows and something must clip. When that clip lived on
    /// `.chg-row` and the buttons were the row's LAST children, the clip ate the
    /// buttons: `docs/adr/0032-daemon-mode-supervised-launcher.md` pushed `+`/`×`
    /// to x=384/414 against a row edge of 347, so a path with a long file name
    /// could not be staged or discarded from the UI at all. `.chg-face` owns the
    /// overflow now and the controls are its siblings.
    ///
    /// Only a browser computes that geometry and CI runs none, so what is pinned
    /// here is the structure the geometry follows from: the face wraps every
    /// descriptive span, closes, and only then come the actions.
    #[test]
    fn a_changes_row_keeps_its_actions_outside_the_clipped_face() {
        let html = include_str!("../assets/ui/index.html");
        let rows: Vec<&str> = html.matches("class=\"chg-row\"").collect();
        assert_eq!(
            rows.len(),
            2,
            "expected the staged and unstaged row templates; the count changed, \
             so re-check that each still keeps its actions outside the face"
        );
        for (i, block) in html.split("class=\"chg-row\"").skip(1).enumerate() {
            let row = block.split_once("</li>").map_or(block, |(head, _)| head);
            let face = row
                .find("class=\"chg-face\"")
                .unwrap_or_else(|| panic!("row {i} must wrap its spans in .chg-face"));
            let act = row
                .find("class=\"chg-act")
                .unwrap_or_else(|| panic!("row {i} must carry at least one .chg-act"));
            assert!(
                face < act,
                "row {i}: the face must open BEFORE the actions — an action inside \
                 the overflow region is an action a long file name can clip away"
            );
            // The face must CLOSE before the first action. Counting `</span>`
            // alone cannot see that — each descriptive child closes itself — so
            // balance the tags: inside the face, every child pairs up, and the ONE
            // unmatched close is the face's own. Equal counts mean the face is
            // still open when the button arrives, which is the bug's exact shape.
            let inner = {
                let after_tag = row[face..]
                    .find('>')
                    .map(|gt| face + gt + 1)
                    .expect("the .chg-face opening tag must close");
                &row[after_tag..act]
            };
            let opens = inner.matches("<span").count();
            let closes = inner.matches("</span>").count();
            assert!(
                inner.contains("chg-name"),
                "row {i}: the file name belongs inside the face; found: {inner:?}"
            );
            assert_eq!(
                closes,
                opens + 1,
                "row {i}: the face must close before the actions — {opens} span(s) \
                 opened and {closes} closed, so the button is INSIDE the clipped \
                 region a long file name overflows; found: {inner:?}"
            );
        }

        // The overflow belongs to the face. `.chg-row` keeps one only as a
        // backstop, and the face is what may shrink (`min-width: 0`).
        let css = include_str!("../assets/ui/styles.css");
        let face = css
            .split_once(".chg-face {")
            .expect("styles.css must define .chg-face")
            .1
            .split_once('}')
            .expect("the .chg-face block must close")
            .0;
        for decl in ["overflow: hidden", "min-width: 0", "flex: 1 1 auto"] {
            assert!(
                face.contains(decl),
                ".chg-face must declare {decl} — it is the clipping, shrinkable \
                 region; found: {face:?}"
            );
        }
    }

    /// The body of one Alpine method in `app.js`, sliced from its opener to the
    /// first four-space-indented `},` — the file's method terminator. Whole-file
    /// `contains` is useless for these pins: `createIcons()` alone appears at
    /// twenty-two sites, so a check that does not scope to the method it is
    /// about passes no matter which one regressed.
    fn js_method_body<'a>(js: &'a str, opener: &str) -> &'a str {
        js.split_once(opener)
            .unwrap_or_else(|| panic!("app.js must define `{opener}`"))
            .1
            .split_once("\n    },")
            .unwrap_or_else(|| panic!("`{opener}` must close at method indent"))
            .0
    }

    /// The body of one CSS rule, sliced from its selector to the closing brace.
    fn css_rule_body<'a>(css: &'a str, selector: &str) -> &'a str {
        css.split_once(selector)
            .unwrap_or_else(|| panic!("styles.css must define `{selector}`"))
            .1
            .split_once('}')
            .unwrap_or_else(|| panic!("the `{selector}` block must close"))
            .0
    }

    /// The one unconditional `createIcons()` runs at `alpine:initialized`, which
    /// is BEFORE either of these reads resolves — so the rows and the panel body
    /// each contain `data-lucide` placeholders that global scan already passed
    /// over, and they stayed blank until an unrelated handler happened to
    /// re-scan the document (#332).
    ///
    /// The project list is bound at the LIST, not at its loader: filtering
    /// rebuilds the `x-for` and blanks every icon again, which a loader-side fix
    /// does not reach. The Runs panel has no such second route, so it converts
    /// from its own read.
    #[test]
    fn the_icons_are_converted_wherever_the_rows_are_built() {
        let html = include_str!("../assets/ui/index.html");
        let list = html
            .split_once("class=\"projects\"")
            .expect("index.html must carry the projects list")
            .1
            // To the first child, NOT to the first `>`: the effect contains an
            // arrow function, whose `=>` would cut the slice in half.
            .split_once("<template")
            .expect("the projects list must hold a row template")
            .0;
        assert!(
            list.contains("x-effect") && list.contains("createIcons()"),
            "`ul.projects` must convert its icons from an effect over its own \
             contents — a loader-side fix leaves the list blank after one \
             keystroke in the search box; found: {list:?}"
        );

        let js = include_str!("../assets/ui/app.js");
        let runs = js_method_body(js, "async hydrateRuns() {");
        assert!(
            runs.contains("createIcons()"),
            "`hydrateRuns` renders the panel body behind an x-if on the data it \
             fetches, so it must convert them itself; found: {runs:?}"
        );
    }

    /// A repo with no `origin` is keyed `path-<hash>` (ADR-0008 D7). That stays
    /// the identity; only the LABEL becomes the directory basename (#332).
    #[test]
    fn a_remoteless_project_is_labelled_by_its_directory() {
        let js = include_str!("../assets/ui/app.js");
        let load = js_method_body(js, "async loadRepos() {");
        assert!(
            load.contains("path: x.path"),
            "`loadRepos` must keep `/api/repos`'s path — the label reads it; \
             found: {load:?}"
        );

        let label = js_method_body(js, "repoLabel(p) {");
        for (needle, why) in [
            (
                r#"startsWith("path-")"#,
                "only a remoteless slug is relabelled",
            ),
            (
                r#"includes("/")"#,
                "a real GitHub repo named `owner/path-utils` must NOT be \
                 relabelled off disk",
            ),
            (
                r"split(/[\\/]/)",
                "both separators — this ships on Windows and Linux",
            ),
            (
                r"replace(/[\\/]+$/",
                "trailing separators go first, or `C:\\src\\widget\\` basenames \
                 to the empty string and the row loses its name",
            ),
        ] {
            assert!(
                label.contains(needle),
                "`repoLabel` must contain {needle:?} — {why}; found: {label:?}"
            );
        }

        let filter = js_method_body(js, "filteredProjects() {");
        assert!(
            filter.contains("repoLabel("),
            "the filter must match the VISIBLE label: typing what the row prints \
             and getting an empty list is the defect a directory label would \
             otherwise introduce; found: {filter:?}"
        );

        let html = include_str!("../assets/ui/index.html");
        assert!(
            html.contains(r#"x-text="repoLabel(p)""#),
            "the row must render the label through `repoLabel`"
        );
        assert!(
            html.contains(r#":title="p.slug""#),
            "the label is a view concern; `.project-slug`'s own title must stay \
             the canonical ADR-0008 D7 slug (the browser tests locate a row by it)"
        );
    }

    /// A twenty-character label in a column fixed at 300px wrapped the row to
    /// two lines (#332).
    #[test]
    fn the_project_name_truncates_instead_of_wrapping() {
        let css = include_str!("../assets/ui/styles.css");
        let body = css_rule_body(css, ".project-slug {");
        for (decl, why) in [
            (
                "flex: 1 1 auto",
                "the name is the row's ONE elastic child — it is what anchors \
                 .chg-badge now the branch chip's `margin-left: auto` has gone",
            ),
            (
                "min-width: 0",
                "a flex item does not shrink below its content without it; that, \
                 not the missing ellipsis, is what produced the wrap",
            ),
            ("overflow: hidden", "the clip the ellipsis needs"),
            ("text-overflow: ellipsis", "the truncation marker"),
            ("white-space: nowrap", "one line"),
        ] {
            assert!(
                body.contains(decl),
                ".project-slug must declare {decl} — {why}; found: {body:?}"
            );
        }
    }

    /// The chip MOVED out of the row and into the Files bar (#332). Neither
    /// block nests a `<div>`, so slicing each to its first `</div>` is exact.
    #[test]
    fn the_branch_chip_lives_in_the_files_bar_not_the_project_row() {
        let html = include_str!("../assets/ui/index.html");
        let slice = |open: &str| -> String {
            html.split_once(open)
                .unwrap_or_else(|| panic!("index.html must carry `{open}`"))
                .1
                .split_once("</div>")
                .unwrap_or_else(|| panic!("the `{open}` block must close"))
                .0
                .to_string()
        };

        let row = slice(r#"class="project-head""#);
        // Proves the slice LANDED. Without it the negative below passes
        // vacuously on any mis-sliced or empty string.
        assert!(
            row.contains("project-slug"),
            "the .project-head slice must contain the project name; found: {row:?}"
        );
        assert!(
            !row.contains("branch-chip"),
            "the branch chip must not sit in the row — capped at 48% of a 300px \
             column it cost the project name half its width; found: {row:?}"
        );
        assert!(
            row.contains("rowTitle(p)"),
            "a collapsed row shows no branch while `filteredProjects` still \
             matches on branch, so it must name the branch in its title; \
             found: {row:?}"
        );

        let files = slice(r#"class="side-head files-sec""#);
        for needle in [
            r#"class="branch-chip""#,
            "openBranchModal(p)",
            "createHere('file')",
        ] {
            assert!(
                files.contains(needle),
                "the Files bar must carry {needle:?} — the chip keeps its \
                 switcher behaviour and the bar keeps its own actions; \
                 found: {files:?}"
            );
        }
        assert_eq!(
            html.matches(r#"class="branch-chip""#).count(),
            1,
            "the chip was MOVED, not copied — two would let the row and the bar \
             disagree about the branch"
        );

        // `.side-head` uppercases and letter-spaces its label; a branch name is
        // case-sensitive, so `feat/UI` would render as a ref that does not exist.
        let css = include_str!("../assets/ui/styles.css");
        let chip = css_rule_body(css, ".files-sec .branch-chip {");
        for decl in [
            "text-transform: none",
            "letter-spacing: normal",
            "max-width: none",
        ] {
            assert!(
                chip.contains(decl),
                ".files-sec .branch-chip must declare {decl}; found: {chip:?}"
            );
        }

        let js = include_str!("../assets/ui/app.js");
        assert!(
            js.contains("rowTitle(p) {"),
            "app.js must define the row's composite title"
        );
    }

    /// The sidebar's left edge was ragged (0.8rem for the headers, search box
    /// and rows; 0.5rem for the Changes toolbar and compose box), so tightening
    /// only the project row would have made it worse. One token, six rules
    /// (#332). `.runs-head` keeps its own value: it is not this column.
    #[test]
    fn the_sidebar_column_keeps_one_gutter() {
        let css = include_str!("../assets/ui/styles.css");
        assert!(
            css.contains("--side-gutter:"),
            "the column's gutter must be a token, so it moves once"
        );
        for selector in [
            ".side-head {",
            ".side-search {",
            ".project-head {",
            ".chg-toolbar {",
            ".side-empty {",
            ".chg-compose {",
        ] {
            let body = css_rule_body(css, selector);
            assert!(
                body.contains("var(--side-gutter)"),
                "`{selector}` shares the sidebar's left edge — a literal value \
                 here re-rags the column; found: {body:?}"
            );
        }
    }
}
