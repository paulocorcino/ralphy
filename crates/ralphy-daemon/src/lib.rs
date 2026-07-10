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
pub mod cookie;
pub mod dispatch;
pub mod identity;
pub mod password;
pub mod protocol;
pub mod registry;
pub mod session;
pub mod totp;
pub mod usage;

use protocol::{Command, Frame, Presence};

/// The daemon's default TCP port. "ralphy" on a phone keypad starts 7-2-5-7.
pub const DEFAULT_PORT: u16 = 7257;

/// The embedded UI, baked in at build time like `assets/prompts` — the daemon
/// reads no files from disk at runtime (ADR-0032 §4).
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
        .route("/login", get(login_page))
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
        .fallback(ui_asset)
        // The auth guard wraps EVERY route above — the API handlers, all three
        // WS upgrades, and the UI fallback — so a network bind rejects an
        // unauthenticated request before it reaches any handler or upgrade.
        .layer(axum::middleware::from_fn_with_state(auth, require_auth))
}

/// Paths reachable WITHOUT a session cookie under a `Session` policy — the login
/// screen and its form endpoint. Everything else falls through to the
/// browser-redirect / `401` logic below.
const LOGIN_ALLOWLIST: &[&str] = &["/login", "/api/login"];

/// The guard over the whole axum surface. First asks the [`auth::AuthPolicy`]
/// (`Localhost` passes all; `Bearer`, and the machine leg of `Session`, pass a
/// correct `Bearer <token>`). Under a `Session` policy a request with no valid
/// bearer is then checked for a browser session cookie; failing that, a top-level
/// `GET` navigation is redirected to `/login` and anything else is `401`.
/// `Localhost`/`Bearer` keep the plain `401` fall-through — fail closed.
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
        // A top-level browser navigation (GET, not an /api or /ws call) gets a
        // redirect to the login screen; API/WS/other verbs fail closed with 401.
        if req.method() == axum::http::Method::GET
            && !path.starts_with("/api")
            && !path.starts_with("/ws")
        {
            return (StatusCode::FOUND, [(header::LOCATION, "/login")]).into_response();
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

/// `GET /ws/command`: one remote command per connection. Read the first frame; a
/// `Frame::Command{verb}` naming a blessed [`dispatch::Verb`] for a registered
/// repo spawns the run and reports two events — an ack (`status:"spawned"` +
/// pid), then the child's exit (`status:"exited"` + code). An unknown verb or an
/// unregistered repo gets one `status:"error"` frame and spawns nothing.
///
/// TEARDOWN INVARIANT (the INVERSE of `session_ws`): the dispatched run keeps its
/// OWN lifecycle. NONE of the `select!` arms — daemon shutdown, client
/// close/error, wait-complete — kills the child; the `Box<dyn dispatch::Child>`
/// has no kill and dropping it does not kill (std semantics). A daemon shutdown
/// or a browser disconnect stops us serving THIS socket but never the run
/// (PRD #157 story 18/20). Do not add a kill to any arm.
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

    let mut child = match dispatch::dispatch(
        &dispatch::ProcessSpawner,
        &dispatch::ralphy_exe(),
        verb,
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
    send_command(
        &mut socket,
        id,
        &cmd.verb,
        serde_json::json!({ "status": "spawned", "pid": pid }),
    )
    .await;

    // `Child::wait` is blocking and must not sit on the tokio runtime.
    let mut wait = tokio::task::spawn_blocking(move || child.wait());
    tokio::select! {
        // Daemon shutdown: stop serving this socket, but LEAVE the run alive.
        _ = shutdown.changed() => {}
        // Client closed or errored: same — abandon the wait, never kill.
        incoming = socket.recv() => {
            let _ = incoming;
        }
        // The run exited: report its code (None → serde null).
        joined = &mut wait => {
            let code = joined.ok().and_then(|r| r.ok()).flatten();
            send_command(
                &mut socket,
                id,
                &cmd.verb,
                serde_json::json!({ "status": "exited", "code": code }),
            )
            .await;
        }
    }
}

/// `GET /api/repos`: the registered repos as JSON, each with its live
/// reachability. Read FRESH from disk on every request so a separate `ralphy
/// run` process's write shows up on the next page refresh. A load error yields
/// an empty list with `200` (logged) rather than failing the page.
async fn repos_route(registry_path: PathBuf) -> Response {
    #[derive(serde::Serialize)]
    struct RepoView {
        slug: String,
        path: String,
        reachable: bool,
    }
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry; serving empty list");
            registry::RegistryStore::default()
        }
    };
    let views: Vec<RepoView> = store
        .repos
        .iter()
        .map(|(slug, entry)| RepoView {
            slug: slug.clone(),
            path: entry.path.clone(),
            reachable: entry.reachable(),
        })
        .collect();
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

/// `GET /login`: the embedded login screen (in the login allowlist, so it is
/// reachable without a cookie under a `Session` policy).
async fn login_page() -> Response {
    match UI.get_file("login.html") {
        Some(file) => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            file.contents(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
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

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
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

    /// The full browser-login round trip under a `Session` policy (issue #179):
    /// no-cookie `401`, `GET /login` `200`, a valid-TOTP `POST /api/login` `200` +
    /// `Set-Cookie`, the cookie authorizes a follow-up, `GET /` redirects to
    /// `/login`, and a machine `Bearer` still authorizes. Plumbing only — the code
    /// itself is pinned by the `totp` RFC-vector unit test.
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

        // 2. The login screen is reachable without a cookie.
        let resp = session_router("tok")
            .oneshot(Request::builder().uri("/login").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "GET /login → 200");

        // 3. A valid current TOTP mints a session cookie.
        let now = now_unix();
        let code = rfc_seed().code_at(now / 30);
        let resp = session_router("tok")
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/login")
                    .header(
                        header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
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
        assert!(set_cookie.contains("ralphy_session="), "cookie name: {set_cookie}");
        assert!(set_cookie.contains("HttpOnly"), "HttpOnly: {set_cookie}");
        assert!(set_cookie.contains("SameSite=Strict"), "SameSite: {set_cookie}");

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

        // 5. A top-level GET with no cookie redirects to the login screen.
        let resp = session_router("tok")
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND, "GET / → 302");
        assert_eq!(
            resp.headers().get(header::LOCATION).and_then(|v| v.to_str().ok()),
            Some("/login"),
            "redirect target is /login"
        );

        // 6. The machine path is unchanged: a correct Bearer still authorizes.
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
        assert_eq!(resp.status(), StatusCode::OK, "Bearer authorizes under Session");
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
