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
pub mod dispatch;
pub mod fswrite;
pub mod identity;
pub mod password;
pub mod protocol;
pub mod registry;
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
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            bind: Ipv4Addr::LOCALHOST.into(),
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
    runtime.block_on(serve(bind_addr(config.bind, config.port)))
}

async fn serve(addr: SocketAddr) -> Result<()> {
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
    // Capture the token as the cookie SIGNING KEY before `for_bind` consumes it and
    // before it is stripped from the env — the Session policy owns this key for the
    // process lifetime, same discipline as today's Bearer.
    let token_for_key = token.clone();
    let policy = auth::AuthPolicy::for_bind(addr.ip(), token)?;
    // Opt-in browser-session hardening (issue #179): a network Bearer bind upgrades
    // to Session ONLY when a TOTP seed is enrolled; with no seed it stays
    // bearer-only, and Localhost is untouched.
    let seed = totp::load_seed()?;
    let pw = password::load()?;
    let policy = auth::upgrade_with_session(policy, token_for_key, seed, pw);
    // INVARIANT: strip the token from the process env on the boot path BEFORE any
    // child can be spawned, so every subsequent `dispatch`/`session` child
    // inherits a token-free env on ALL paths (mirrors RALPHY_EVENTS_TOKEN,
    // ADR-0019). The policy already holds the effective token.
    auth::strip_token_from_env();

    // Resolve the registry path once and hand it to the router. `/api/repos`
    // reads it FRESH from disk on each request, so the resident daemon sees
    // writes made by separate `ralphy run` processes (ADR-0032).
    let registry_path = registry::repos_toml_path()?;
    let usage_dir = usage::usage_dir_path()?;
    let claude_projects_dir = usage::claude_projects_dir_path()?;
    let codex_dir = usage::codex_dir_path()?;
    let opencode_db = usage::opencode_db_path()?;
    let kimi_dir = usage::kimi_dir_path()?;
    let kimi_code_dir = usage::kimi_code_dir_path()?;
    axum::serve(
        listener,
        router(
            id,
            registry_path,
            usage_dir,
            claude_projects_dir,
            codex_dir,
            opencode_db,
            kimi_dir,
            kimi_code_dir,
            start,
            shutdown_rx,
            policy,
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

/// The daemon's HTTP surface. Real routes sit *before* the embedded-UI
/// fallback. `GET /api/identity` returns the loaded identity as JSON, or 404
/// when the daemon is un-baptized, so the static page can render "avatar name"
/// at runtime (the embedded HTML bakes in no identity).
// One positional per resolved-at-boot path/handle; grouping them into a struct
// would only move the argument list, not shrink it, and churn the ~20 call sites.
#[allow(clippy::too_many_arguments)]
pub fn router(
    identity: Option<identity::Identity>,
    registry_path: PathBuf,
    usage_dir: PathBuf,
    claude_projects_dir: PathBuf,
    codex_dir: PathBuf,
    opencode_db: PathBuf,
    kimi_dir: PathBuf,
    kimi_code_dir: PathBuf,
    start: Instant,
    shutdown: tokio::sync::watch::Receiver<bool>,
    auth: auth::AuthPolicy,
) -> Router {
    let ws_identity = identity.clone();
    // The session manager owns sessions for this router's lifetime (the tmux
    // model, issue #166). Constructed here — NOT a `router` parameter — so the
    // public `router` signature and its ~20 call sites are untouched; production
    // calls `router` exactly once, so one manager per router is correct.
    let sessions = Arc::new(session::SessionManager::new());
    // `shutdown` is consumed by the `/ws` presence closure; clone one for the
    // session route so a live session bridge also stops serving on graceful
    // shutdown (it detaches, never closing the session).
    let session_shutdown = shutdown.clone();
    let session_registry = registry_path.clone();
    // The live file-tree watcher (#196) is shared across every `/ws/tree`
    // connection for this router's lifetime — same ownership model as `sessions`,
    // constructed here (NOT a `router` param) so the ~20-call-site signature holds.
    let watchers = Arc::new(watch::WatcherManager::new(watch::MAX_WATCHES));
    let tree_watchers = watchers.clone();
    let tree_registry = registry_path.clone();
    let tree_shutdown = shutdown.clone();
    // A dispatched run must survive daemon shutdown (inverse of the session
    // invariant), but the handler still watches `shutdown` to stop serving the
    // socket — it just never kills the child. Clone one for that route.
    let command_shutdown = shutdown.clone();
    let command_registry = registry_path.clone();
    // The daemon identity a dispatched child inherits as RALPHY_DAEMON_ID (#168):
    // captured here BEFORE `identity` is moved into the `/api/identity` closure.
    // Only the dispatch path passes it; session/console children get none.
    let command_daemon_id = identity.as_ref().map(|i| i.id.to_string());
    // The daemon identity served on `/api/usage` responses: captured here BEFORE
    // `identity` is moved into the `/api/identity` closure (mirrors
    // `command_daemon_id` above).
    let usage_daemon_id = identity.as_ref().map(|i| i.id.to_string());
    // The login form needs the auth policy (the `Session`'s TOTP/password/token)
    // to validate a code and sign a cookie. Cloned here BEFORE `auth` is moved
    // into the guard layer below; `Session` is `Arc`-cheap, `Bearer`/`Localhost`
    // trivially so.
    let login_auth = auth.clone();
    Router::new()
        .route("/api/identity", get(move || identity_route(identity)))
        .route(
            "/api/repos",
            get({
                let p = registry_path.clone();
                move || repos_route(p)
            }),
        )
        .route(
            "/api/usage",
            get({
                let dir = usage_dir.clone();
                let claude_dir = claude_projects_dir.clone();
                let codex_dir = codex_dir.clone();
                let opencode_db = opencode_db.clone();
                let kimi_dir = kimi_dir.clone();
                let kimi_code_dir = kimi_code_dir.clone();
                let registry = registry_path.clone();
                let daemon_id = usage_daemon_id.clone();
                move |q: Query<UsageQuery>| {
                    usage_route(
                        dir,
                        claude_dir,
                        codex_dir,
                        opencode_db,
                        kimi_dir,
                        kimi_code_dir,
                        registry,
                        daemon_id,
                        q.0.since,
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
                async move {
                    ws.on_upgrade(move |socket| {
                        command_ws(socket, registry_path, shutdown, daemon_id)
                    })
                }
            }),
        )
        .route(
            "/ws/tree",
            get(move |ws: WebSocketUpgrade| {
                let watchers = tree_watchers.clone();
                let registry_path = tree_registry.clone();
                let shutdown = tree_shutdown.clone();
                async move {
                    ws.on_upgrade(move |socket| tree_ws(socket, watchers, registry_path, shutdown))
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
                move |headers: axum::http::HeaderMap| {
                    let auth = auth.clone();
                    async move { session_state_route(auth, headers).await }
                }
            }),
        )
        .route("/api/logout", post(logout_route))
        .route("/api/security/state", get(security_state_route))
        .route(
            "/api/security/totp/enroll",
            post(security_totp_enroll_route),
        )
        .route(
            "/api/security/totp/revoke",
            post(security_totp_revoke_route),
        )
        .route("/api/security/password", post(security_password_route))
        .route(
            "/api/security/token/remint",
            post(security_token_remint_route),
        )
        .route(
            "/api/security/require-login",
            post(security_require_login_route),
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
    State(policy): State<auth::AuthPolicy>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
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
            return next.run(req).await;
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
/// (home dir when absent); a REATTACH carries `id` (and optional `takeover=1`).
/// All optional so one struct serves every shape; the handler dispatches on
/// `id` first, then `console`.
#[derive(serde::Deserialize)]
struct SessionQuery {
    repo: Option<String>,
    agent: Option<String>,
    id: Option<u64>,
    takeover: Option<u32>,
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

/// `GET /ws/session`: three shapes over one route.
///
/// - `?id=<id>[&takeover=1]` — REATTACH to a daemon-owned session. `attach`
///   returns `404` for an unknown id and `409` for a busy one (a single writer is
///   attached and `takeover` was not set) — both BEFORE the upgrade, so a refusal
///   is an HTTP status the browser can read, not a silently-dropped socket.
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
    let notified = evict.notified();
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
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = &mut notified => break, // taken over, or the child exited
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
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
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
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    daemon_id: Option<String>,
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
    let slug = cmd
        .payload
        .get("repo")
        .and_then(|v| v.as_str())
        .unwrap_or("");
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
    let Some(entry) = store.entry(slug) else {
        send_command(
            &mut socket,
            id,
            &cmd.verb,
            serde_json::json!({ "status": "error", "message": "unknown repo" }),
        )
        .await;
        return;
    };

    // Observe verbs (ADR-0036 §2) read repo state IN-DAEMON and answer on THIS
    // `id` — they NEVER reach the spawn path below. The branch always sends
    // exactly one reply (ok or error) and returns; confinement (`tree`/`confine`)
    // is the security boundary, so an out-of-root read looks like a plain miss.
    if verb.effect_class() == dispatch::EffectClass::Observe {
        let root = Path::new(&entry.path);
        let rel = cmd
            .payload
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let payload = match verb {
            dispatch::Verb::TreeList => match tree::list(root, rel) {
                Ok(entries) => serde_json::json!({ "status": "ok", "entries": entries }),
                Err(_) => serde_json::json!({ "status": "error", "reason": "not found" }),
            },
            dispatch::Verb::FileRead => match tree::read(root, rel) {
                Ok(content) => serde_json::json!({ "status": "ok", "content": content }),
                Err(e) => {
                    let reason = match e {
                        tree::ReadError::Binary => "binary",
                        tree::ReadError::TooLarge => "too large",
                        tree::ReadError::NotFound => "not found",
                    };
                    serde_json::json!({ "status": "error", "reason": reason })
                }
            },
            // Unreachable: only TreeList/FileRead are Observe verbs.
            _ => serde_json::json!({ "status": "error", "reason": "refused" }),
        };
        send_command(&mut socket, id, &cmd.verb, payload).await;
        return;
    }

    // Write verbs (ADR-0036 Write amendment) perform a confined byte-op IN-DAEMON
    // and answer on THIS `id` — they NEVER spawn and NEVER consult the run lock.
    // Confinement (`fswrite`/`confine`) is the security boundary; a write-escape
    // refusal surfaces verbatim as `refused` (unlike reads, which mask to a miss —
    // a write-escape confirms nothing).
    if verb.effect_class() == dispatch::EffectClass::Write {
        let root = Path::new(&entry.path);
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
                fswrite::write(root, rel, content)
            }
            dispatch::Verb::FileCreate => {
                let dir = cmd
                    .payload
                    .get("dir")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                fswrite::create(root, rel, dir)
            }
            dispatch::Verb::FileRename => {
                let to = cmd.payload.get("to").and_then(|v| v.as_str()).unwrap_or("");
                fswrite::rename(root, rel, to)
            }
            dispatch::Verb::FileDelete => fswrite::delete(root, rel),
            // Unreachable: only the four file.* verbs are Write verbs.
            _ => Err(fswrite::WriteError::Io),
        };
        let payload = match result {
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
        };
        send_command(&mut socket, id, &cmd.verb, payload).await;
        return;
    }

    // Query verbs (ADR-0036 §2) spawn-and-COLLECT a read-only CLI invocation and
    // answer ONCE on THIS id — no live stream. The verb picks BOTH the argv and
    // the reply field: `config.get`→`config get --json`/`config`,
    // `board.list`→`issues --format json --board`/`board`,
    // `issue.show`→`issues show <n> --format json`/`issue`. The parsed JSON rides
    // that field; a non-JSON stdout falls back to a raw string.
    if verb.effect_class() == dispatch::EffectClass::Query {
        let (argv_result, field): (Result<Vec<String>, dispatch::ArgvError>, &str) = match verb {
            dispatch::Verb::ConfigGet => (dispatch::config_argv(verb, &cmd.payload), "config"),
            dispatch::Verb::BoardList => (Ok(dispatch::board_argv()), "board"),
            dispatch::Verb::IssueShow => (dispatch::issue_show_argv(&cmd.payload), "issue"),
            dispatch::Verb::BranchList => (Ok(dispatch::branch_list_argv()), "branches"),
            // Unreachable: only the four Query verbs reach this branch.
            _ => (Err(dispatch::ArgvError::BadParam("verb")), "config"),
        };
        let payload = match argv_result {
            Err(e) => {
                tracing::warn!(error = %e, "refused a query with invalid params");
                serde_json::json!({ "status": "error", "message": "invalid query options" })
            }
            Ok(argv) => {
                match collect_config(argv, PathBuf::from(&entry.path), daemon_id.clone()).await {
                    Some((Some(0), bytes)) => {
                        let text = String::from_utf8_lossy(&bytes);
                        let parsed: serde_json::Value = serde_json::from_str(text.trim())
                            .unwrap_or_else(|_| serde_json::Value::String(text.trim().to_string()));
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
        };
        send_command(&mut socket, id, &cmd.verb, payload).await;
        return;
    }

    // Mutate verbs (ADR-0036 §2/§6) spawn-and-collect a run-lock-aware write
    // (`config set`/`config unset`) and answer once; a non-zero exit (the CLI's
    // run-lock refusal or unknown-key error) relays as the trimmed stderr.
    if verb.effect_class() == dispatch::EffectClass::Mutate {
        let argv_result = match verb {
            dispatch::Verb::ConfigSet | dispatch::Verb::ConfigUnset => {
                dispatch::config_argv(verb, &cmd.payload)
            }
            dispatch::Verb::BranchSwitch | dispatch::Verb::BranchCreate => {
                dispatch::branch_argv(verb, &cmd.payload)
            }
            dispatch::Verb::LabelSet => dispatch::label_argv(&cmd.payload),
            _ => Err(dispatch::ArgvError::BadParam("verb")),
        };
        let payload = match argv_result {
            Err(e) => {
                tracing::warn!(error = %e, "refused a mutation with invalid params");
                serde_json::json!({ "status": "error", "message": "invalid mutation options" })
            }
            Ok(argv) => {
                match collect_config(argv, PathBuf::from(&entry.path), daemon_id.clone()).await {
                    Some((Some(0), _)) => serde_json::json!({ "status": "ok" }),
                    Some((_, bytes)) => {
                        let msg = String::from_utf8_lossy(&bytes);
                        let msg = msg.trim();
                        let msg = if msg.is_empty() { "refused" } else { msg };
                        serde_json::json!({ "status": "error", "message": msg })
                    }
                    None => {
                        serde_json::json!({ "status": "error", "message": "mutation write failed" })
                    }
                }
            }
        };
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
    let mut wait = tokio::task::spawn_blocking(move || child.wait());
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
/// TEARDOWN INVARIANT: on EVERY exit path — daemon shutdown OR client close/error
/// — the connection releases EVERY dir it watched (tracked in `watched`) so the
/// last release tears the repo watcher down, and aborts its forwarder tasks. A
/// leaked watch would keep an OS watcher (and its debouncer thread) alive forever.
async fn tree_ws(
    mut socket: WebSocket,
    watchers: Arc<watch::WatcherManager>,
    registry_path: PathBuf,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    // Fan-in: one forwarder task per subscribed repo pipes that repo's broadcast
    // into this shared channel, so the select! loop watches ONE receiver regardless
    // of how many repos/dirs the connection holds. Keyed by repo so it is torn down
    // when this connection releases the repo's LAST dir — and re-spawned (on the
    // fresh broadcast the manager rebuilds) if the same repo is watched again.
    let (nudge_tx, mut nudge_rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    let mut forwarders: std::collections::BTreeMap<String, tokio::task::JoinHandle<()>> =
        std::collections::BTreeMap::new();
    // The (repo, rel) dirs THIS connection holds, in normalized form — held at most
    // once each (a duplicate `watch` is a no-op, so the manager refcount this
    // connection contributes stays 1 per dir and teardown releases it exactly once).
    // Doubles as the per-connection push filter (a repo's broadcast carries every
    // dir, including ones other connections watch).
    let mut watched: Vec<(String, String)> = Vec::new();

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Binary(bytes))) => {
                    let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) else {
                        continue;
                    };
                    let repo = cmd
                        .payload
                        .get("repo")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let rel = watch::norm_rel(
                        cmd.payload.get("path").and_then(|v| v.as_str()).unwrap_or(""),
                    );
                    match cmd.verb.as_str() {
                        "watch" => {
                            if repo.is_empty() {
                                continue;
                            }
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
                        "unwatch" => {
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
    }

    // TEARDOWN: release every held dir (the last release tears the watcher down)
    // and abort the forwarders (their broadcast receivers may otherwise outlive us
    // if another connection keeps the repo alive).
    for (repo, rel) in &watched {
        watchers.unwatch(repo, rel);
    }
    for (_repo, forwarder) in forwarders {
        forwarder.abort();
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
// One positional per resolved-at-boot store path; grouping them would only move
// the argument list, not shrink it (mirrors `router`).
#[allow(clippy::too_many_arguments)]
async fn usage_route(
    usage_dir: PathBuf,
    claude_projects_dir: PathBuf,
    codex_dir: PathBuf,
    opencode_db: PathBuf,
    kimi_dir: PathBuf,
    kimi_code_dir: PathBuf,
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
    let interactive = usage::interactive_records(
        &claude_projects_dir,
        &codex_dir,
        &opencode_db,
        &kimi_dir,
        &kimi_code_dir,
        &store,
        &runs,
        since.as_deref(),
    );
    Json(serde_json::json!({
        "daemon_id": daemon_id,
        "records": runs,
        "interactive": interactive,
    }))
    .into_response()
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

/// The `POST /api/login` form: the current TOTP `code` and, when a password is
/// enrolled, the operator's `password`. `password` is `Option` so a bind with no
/// password enrolled accepts a form carrying only `code`.
#[derive(serde::Deserialize)]
struct LoginForm {
    code: String,
    password: Option<String>,
}

/// `POST /api/login`: validate the TOTP code (and password, if enrolled) against
/// the `Session` policy. On success `200` + a `Set-Cookie: ralphy_session=…`
/// header; on a bad credential `401`. Login is meaningless without a network
/// `Session` policy, so any other policy returns `404`.
async fn login_submit(auth: auth::AuthPolicy, Form(form): Form<LoginForm>) -> Response {
    let auth::AuthPolicy::Session(session) = &auth else {
        return (StatusCode::NOT_FOUND, "login not enabled").into_response();
    };
    match session.login(&form.code, form.password.as_deref(), now_unix()) {
        Some(cookie) => (
            StatusCode::OK,
            [(header::SET_COOKIE, cookie::set_cookie_value(&cookie))],
        )
            .into_response(),
        None => (StatusCode::UNAUTHORIZED, "invalid credentials").into_response(),
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

/// `POST /api/logout`: emit a `Max-Age=0` clearing `Set-Cookie`. The session
/// cookie is `HttpOnly`, so JS cannot clear it — the server must (issue #186).
async fn logout_route() -> Response {
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
}

/// `GET /api/session`: report whether this request is authorized and whether a
/// password factor is enrolled. `Localhost`/`Bearer` are always `authed`; under a
/// `Session` policy `authed` reflects a valid `Bearer` OR session cookie.
async fn session_state_route(auth: auth::AuthPolicy, headers: axum::http::HeaderMap) -> Response {
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
    })
    .into_response()
}

/// The daemon's auth-state surface for the Security modal (issue #195): which
/// factors are enrolled in the REAL stores. `require_login` is DERIVED from TOTP
/// enrolment (a network bind with a seed already forces `Session`; localhost
/// never requires login), so there is no separate flag file (ADR-0032 §4).
#[derive(serde::Serialize)]
struct SecurityState {
    token_set: bool,
    password_set: bool,
    totp_enrolled: bool,
    require_login: bool,
}

/// Read the real store FILES under `dir` and report enrolment. Path-explicit (no
/// env reads) so tests pass a tempdir. `require_login == totp_enrolled`.
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
        require_login: totp_enrolled,
    }
}

/// Enroll (mint-once) a TOTP seed under `dir`; return its `otpauth://` URI and
/// whether it was newly minted. The URI is shown once (QR + base32); a second
/// enrol returns the SAME secret with `newly_minted=false`.
fn enroll_totp_at(dir: &Path) -> Result<(String, bool)> {
    let (seed, newly_minted) = totp::ensure_seed_at(&totp::seed_path_in(dir))?;
    Ok((seed.otpauth_uri("ralphy", "daemon"), newly_minted))
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

/// The require-login gate: enabling require-login demands an enrolled TOTP seed
/// (localhost stays frictionless; a network bind with a seed already forces
/// `Session`). `Err("totp not enrolled")` when enabling with no seed; else `Ok`
/// — state is derived from the seed, so this validates rather than stores a flag.
fn require_login_at(dir: &Path, enable: bool) -> Result<()> {
    if enable
        && totp::load_seed_from(&totp::seed_path_in(dir))
            .ok()
            .flatten()
            .is_none()
    {
        anyhow::bail!("totp not enrolled");
    }
    Ok(())
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

/// `POST /api/security/totp/revoke`: delete the seed (mint-once posture — a later
/// enrol mints a fresh one).
async fn security_totp_revoke_route() -> Response {
    match auth::store_dir().and_then(|dir| totp::revoke_seed_at(&totp::seed_path_in(&dir))) {
        Ok(()) => Json(serde_json::json!({ "revoked": true })).into_response(),
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
async fn security_password_route(Form(form): Form<PasswordForm>) -> Response {
    match auth::store_dir().and_then(|dir| set_password_at(&dir, form.password.as_deref())) {
        Ok(password_set) => {
            Json(serde_json::json!({ "password_set": password_set })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to update the password");
            (StatusCode::INTERNAL_SERVER_ERROR, "password update failed").into_response()
        }
    }
}

/// `POST /api/security/token/remint`: rotate the access token (never echoed).
async fn security_token_remint_route() -> Response {
    match auth::store_dir().and_then(|dir| remint_token_at(&dir)) {
        Ok(()) => Json(serde_json::json!({ "reminted": true })).into_response(),
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

/// `POST /api/security/require-login`: the server-side gate for AC4 — enabling
/// require-login without an enrolled TOTP seed is refused (`400`). Enabling with a
/// seed (or disabling) is `Ok`; the state itself stays derived from the seed.
async fn security_require_login_route(Form(form): Form<RequireLoginForm>) -> Response {
    match auth::store_dir().and_then(|dir| require_login_at(&dir, form.enable)) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
        )
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
    }

    #[test]
    fn security_state_reflects_the_stores() {
        let dir = tempfile::tempdir().unwrap();
        // Empty store → every factor unset.
        let s = security_state_at(dir.path());
        assert!(!s.token_set && !s.password_set && !s.totp_enrolled && !s.require_login);
        // Writing a seed flips totp_enrolled AND the derived require_login.
        totp::save_seed_to(&totp::generate_seed(), &totp::seed_path_in(dir.path())).unwrap();
        let s = security_state_at(dir.path());
        assert!(
            s.totp_enrolled && s.require_login,
            "seed → enrolled + require"
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
        assert!(
            body.contains("ralphy daemon"),
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
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
            claude_dir.path().to_path_buf(),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
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
            PathBuf::from("does-not-exist"),
            codex_dir.path().to_path_buf(),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            db.clone(),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            kimi_dir.path().to_path_buf(),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Bearer("tok".into()),
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Bearer("tok".into()),
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Localhost,
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Bearer("tok".into()),
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
        let policy = auth::AuthPolicy::Session(std::sync::Arc::new(auth::SessionAuth {
            token: token.to_string(),
            totp: rfc_seed(),
            password: None,
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            policy,
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
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            Instant::now(),
            idle_shutdown(),
            auth::AuthPolicy::Bearer("tok".into()),
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
    /// true of the pre-#205 mock login) and instead explains the loopback
    /// login-gate exemption inline (issue #205, audit finding AC5).
    #[tokio::test]
    async fn login_gate_drops_mock_hint() {
        let shell = body_string(get_local("/").await).await;
        assert!(
            !shell.contains("any 6-digit code works"),
            "mock hint must be gone"
        );
        assert!(
            shell.contains("the login gate only applies to a network bind with TOTP"),
            "loopback explanation must be present"
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
                PathBuf::from("does-not-exist"),
                PathBuf::from("does-not-exist"),
                PathBuf::from("does-not-exist"),
                PathBuf::from("does-not-exist"),
                PathBuf::from("does-not-exist"),
                Instant::now(),
                idle_shutdown(),
                auth::AuthPolicy::Bearer("tok".into()),
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
}
