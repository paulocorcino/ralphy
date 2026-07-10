//! Ralphy's resident daemon (docs/adr/0032): a foreground HTTP listener bound
//! to localhost, serving the embedded workbench UI. This is the tracer bullet —
//! no sessions, no command vocabulary yet — but the shape is the decided one:
//! a library crate wired by `ralphy-cli`, the workspace's async runtime (tokio +
//! axum) confined here, runs reached only by spawning `ralphy` processes (never
//! by importing the core).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::Query;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use include_dir::{include_dir, Dir};

pub mod dispatch;
pub mod identity;
pub mod protocol;
pub mod registry;
pub mod session;

use protocol::{Frame, Presence};

/// The daemon's default TCP port. "ralphy" on a phone keypad starts 7-2-5-7.
pub const DEFAULT_PORT: u16 = 7257;

/// The embedded UI, baked in at build time like `assets/prompts` — the daemon
/// reads no files from disk at runtime (ADR-0032 §4).
static UI: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/ui");

/// What the composition root decides; everything else is the daemon's.
pub struct DaemonConfig {
    /// TCP port for the listener. The interface is not configurable in this
    /// slice: the daemon binds `127.0.0.1` only — a non-localhost bind is a
    /// future explicit opt-in that requires a bearer token (ADR-0032 §4).
    pub port: u16,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self { port: DEFAULT_PORT }
    }
}

/// The loopback-only bind address (ADR-0032 §4: local listener first,
/// inbound never). Centralized so no call site constructs a wider bind.
pub fn bind_addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// Run the daemon in the foreground until Ctrl+C. Blocking on purpose: the
/// tokio runtime is created and dropped inside, so callers (the sync CLI)
/// never see async types.
pub fn run(config: DaemonConfig) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building the daemon's tokio runtime")?;
    runtime.block_on(serve(bind_addr(config.port)))
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
    // Resolve the registry path once and hand it to the router. `/api/repos`
    // reads it FRESH from disk on each request, so the resident daemon sees
    // writes made by separate `ralphy run` processes (ADR-0032).
    let registry_path = registry::repos_toml_path()?;
    axum::serve(listener, router(id, registry_path, start, shutdown_rx))
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
) -> Router {
    let ws_identity = identity.clone();
    // `shutdown` is consumed by the `/ws` presence closure; clone one for the
    // session route so a live session also tears down on graceful shutdown.
    let session_shutdown = shutdown.clone();
    let session_registry = registry_path.clone();
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
            get(move |ws: WebSocketUpgrade, q: Query<SessionQuery>| {
                let registry_path = session_registry.clone();
                let shutdown = session_shutdown.clone();
                async move { session_ws_upgrade(ws, q, registry_path, shutdown).await }
            }),
        )
        .fallback(ui_asset)
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

/// Query for `/ws/session`: which registered `repo` slug and which `agent`.
#[derive(serde::Deserialize)]
struct SessionQuery {
    repo: String,
    agent: String,
}

/// `GET /ws/session?repo=<slug>&agent=<claude|codex|opencode>`: resolve the repo
/// and agent, then upgrade to a WebSocket bridging the codec to a live PTY
/// session. Rejects (`400`) an unknown agent, an unreadable registry, or an
/// unregistered slug BEFORE upgrading, so a bad request fails as HTTP, not as a
/// silently-dropped socket.
async fn session_ws_upgrade(
    ws: WebSocketUpgrade,
    Query(query): Query<SessionQuery>,
    registry_path: PathBuf,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Response {
    let Some(agent) = session::Agent::from_query(&query.agent) else {
        return (StatusCode::BAD_REQUEST, "unknown agent").into_response();
    };
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry for a session");
            return (StatusCode::BAD_REQUEST, "repo registry unreadable").into_response();
        }
    };
    let Some(entry) = store.entry(&query.repo) else {
        return (StatusCode::BAD_REQUEST, "unknown repo").into_response();
    };
    let spec = session::spec_for(agent, PathBuf::from(&entry.path), 24, 80);
    ws.on_upgrade(move |socket| session_ws(socket, spec, shutdown))
}

/// Bridge one WebSocket to one PTY session: PTY output → `Frame::Terminal`
/// binary messages; client `Frame::Terminal` → PTY stdin; client
/// `Frame::Command{verb:"resize"}` → PTY resize. The loop breaks on client
/// close/error, child EOF, a send failure, OR daemon shutdown.
///
/// TEARDOWN INVARIANT: exactly one `session.close()` runs on EVERY exit path.
/// There is no `?`/early `return` between spawn and the post-loop `close()`, so a
/// live session can neither leak its child tree nor stall graceful shutdown
/// (mirrors `ws_presence_loop`'s shutdown arm; #161 friction c2c44b5).
async fn session_ws(
    mut socket: WebSocket,
    spec: session::SessionSpec,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut session = match session::Session::spawn(spec) {
        Ok(session) => session,
        Err(e) => {
            // Spawn failed → no session to close; report and drop the socket.
            tracing::warn!(error = %e, "failed to spawn a workbench session");
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };
    let mut output = session.take_output();
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            chunk = output.recv() => match chunk {
                Some(bytes) => {
                    let frame = Frame::Terminal { session: 1, data: bytes };
                    if socket
                        .send(Message::Binary(protocol::encode(&frame).into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                None => break, // the child tree exited
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Binary(bytes))) => match protocol::decode(&bytes) {
                    Ok(Frame::Terminal { data, .. }) => {
                        if session.write(&data).is_err() {
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
                            let _ = session.resize(rows, cols);
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
    session.close();
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

        let resp = router(None, registry_path, Instant::now(), idle_shutdown())
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
    fn bind_addr_is_loopback_only() {
        let addr = bind_addr(DEFAULT_PORT);
        assert!(addr.ip().is_loopback(), "default bind must be 127.0.0.1");
        assert_eq!(addr.port(), DEFAULT_PORT);
    }
}
