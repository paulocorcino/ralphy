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
use axum::extract::{Query, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use include_dir::{include_dir, Dir};

pub mod auth;
pub mod autostart;
pub mod dispatch;
pub mod identity;
pub mod protocol;
pub mod registry;
pub mod session;

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
    let policy = auth::AuthPolicy::for_bind(addr.ip(), token)?;
    // INVARIANT: strip the token from the process env on the boot path BEFORE any
    // child can be spawned, so every subsequent `dispatch`/`session` child
    // inherits a token-free env on ALL paths (mirrors RALPHY_EVENTS_TOKEN,
    // ADR-0019). The policy already holds the effective token.
    auth::strip_token_from_env();

    // Resolve the registry path once and hand it to the router. `/api/repos`
    // reads it FRESH from disk on each request, so the resident daemon sees
    // writes made by separate `ralphy run` processes (ADR-0032).
    let registry_path = registry::repos_toml_path()?;
    axum::serve(
        listener,
        router(id, registry_path, start, shutdown_rx, policy),
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
pub fn router(
    identity: Option<identity::Identity>,
    registry_path: PathBuf,
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
                async move {
                    ws.on_upgrade(move |socket| command_ws(socket, registry_path, shutdown))
                }
            }),
        )
        .fallback(ui_asset)
        // The auth guard wraps EVERY route above — the API handlers, all three
        // WS upgrades, and the UI fallback — so a network bind rejects an
        // unauthenticated request before it reaches any handler or upgrade.
        .layer(axum::middleware::from_fn_with_state(auth, require_auth))
}

/// The bearer-token guard over the whole axum surface. Reads
/// `Authorization` and asks the [`auth::AuthPolicy`]; on refusal returns `401`
/// without running the inner handler. A `Localhost` policy authorizes every
/// request unconditionally, so the loopback path is unaffected.
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
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response()
    }
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

/// Query for `/ws/session`. A NEW launch carries `repo` + `agent`; a REATTACH
/// carries `id` (and optional `takeover=1`). All optional so one struct serves
/// both shapes; the handler dispatches on `id`.
#[derive(serde::Deserialize)]
struct SessionQuery {
    repo: Option<String>,
    agent: Option<String>,
    id: Option<u64>,
    takeover: Option<u32>,
}

/// Query for `POST /api/sessions/close`: which session to end.
#[derive(serde::Deserialize)]
struct CloseQuery {
    id: u64,
}

/// `GET /ws/session`: two shapes over one route.
///
/// - `?id=<id>[&takeover=1]` — REATTACH to a daemon-owned session. `attach`
///   returns `404` for an unknown id and `409` for a busy one (a single writer is
///   attached and `takeover` was not set) — both BEFORE the upgrade, so a refusal
///   is an HTTP status the browser can read, not a silently-dropped socket.
/// - `?repo=<slug>&agent=<claude|codex|opencode>` — NEW launch. Rejects (`400`)
///   an unknown agent, an unreadable registry, or an unregistered slug before
///   upgrading; a spawn failure is `500`.
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
            Err(session::AttachError::Busy) => (StatusCode::CONFLICT, "session busy").into_response(),
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
    match sessions.spawn_attached(repo.to_string(), agent_str.to_string(), spec) {
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
    // Pin ONE eviction future across the whole loop: `notify_waiters` only wakes
    // currently-registered waiters, so a fresh `notified()` per iteration could
    // miss an eviction that fires mid-iteration and leak the single-writer slot.
    let evict = attach.evict.clone();
    let notified = evict.notified();
    tokio::pin!(notified);
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

    /// The auth layer covers the REMOTE-EXEC WS routes, not just `/api`: an
    /// unauthenticated `/ws/session` (PTY) and `/ws/command` (run dispatch)
    /// request is rejected `401` by the middleware BEFORE reaching the upgrade
    /// handler. A `401` here (not the handler's `400`) proves the layer fired, so
    /// a future route reordered past the layer would fail this test instead of
    /// silently serving an unauthenticated shell/run trigger.
    #[tokio::test]
    async fn bearer_policy_gates_the_remote_exec_ws_routes() {
        for uri in ["/ws/session?repo=x&agent=claude", "/ws/command"] {
            let resp = router(
                None,
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
