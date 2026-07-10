//! Ralphy's resident daemon (docs/adr/0032): a foreground HTTP listener bound
//! to localhost, serving the embedded workbench UI. This is the tracer bullet —
//! no sessions, no command vocabulary yet — but the shape is the decided one:
//! a library crate wired by `ralphy-cli`, the workspace's async runtime (tokio +
//! axum) confined here, runs reached only by spawning `ralphy` processes (never
//! by importing the core).

use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use include_dir::{include_dir, Dir};

pub mod identity;

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
    axum::serve(listener, router(id))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving the daemon listener")?;
    tracing::info!("daemon stopped");
    Ok(())
}

/// The daemon's HTTP surface. Real routes sit *before* the embedded-UI
/// fallback. `GET /api/identity` returns the loaded identity as JSON, or 404
/// when the daemon is un-baptized, so the static page can render "avatar name"
/// at runtime (the embedded HTML bakes in no identity).
pub fn router(identity: Option<identity::Identity>) -> Router {
    Router::new()
        .route("/api/identity", get(move || identity_route(identity)))
        .fallback(ui_asset)
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

    async fn get(path: &str) -> Response {
        router(None)
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
        let resp = router(Some(id))
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

        let resp = router(None)
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
    fn bind_addr_is_loopback_only() {
        let addr = bind_addr(DEFAULT_PORT);
        assert!(addr.ip().is_loopback(), "default bind must be 127.0.0.1");
        assert_eq!(addr.port(), DEFAULT_PORT);
    }
}
