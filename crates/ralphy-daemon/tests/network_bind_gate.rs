//! The cross-site gate for remote access (docs/adr/0032 §4): the browser that
//! reaches the daemon at its bound address — or at a declared name — passes, and
//! everything else is still refused.
//!
//! This is the seam §4 exists for, and it covers BOTH remote shapes, which invert
//! each other: a non-loopback `--bind` reached directly, and a reverse tunnel (dev
//! tunnels, ngrok) whose agent dials out to a daemon still on loopback. A gate that
//! names the daemon by loopback refuses the operator's browser on every route and
//! every WS upgrade in both — the workbench is unreachable from another machine
//! rather than merely degraded. The unit tests on the predicates cannot see that,
//! because the failure is in the middleware that runs BEFORE the auth policy. Hence
//! end-to-end, through `require_auth`, via `ServiceExt::oneshot` (no socket: the
//! gate reads the bound address the auth state reports, not where the test listens).
//!
//! SOLE env-setter in its file: `RALPHY_DAEMON_DIR` is process-global, so this
//! env-setting test must be alone in its file (no intra-process race).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Instant;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use ralphy_daemon::auth::AuthState;
use ralphy_daemon::epoch::SessionEpoch;
use ralphy_daemon::router;
use tower::ServiceExt;

const TOKEN: &str = "n3twork-token";
const BOUND: &str = "100.64.0.1:7257";
const LOOPBACK: &str = "127.0.0.1:7257";
const DECLARED: &str = "desk.example.net";
/// A reverse tunnel's public hostname, the shape a dev-tunnel/ngrok endpoint has.
const TUNNEL: &str = "abc123-7257.use.devtunnels.ms";

/// A fresh router each call (`oneshot` consumes it), whose auth state reports the
/// given bind plus one declared host.
fn fresh_router(bound: &str) -> axum::Router {
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    let bound: SocketAddr = bound.parse().expect("a valid bound address");
    let auth = AuthState::boot(
        bound,
        Some(TOKEN.to_string()),
        SessionEpoch::in_memory_detached(),
        &[DECLARED.to_string(), TUNNEL.to_string()],
    )
    .expect("a bind with a token must boot");
    router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        auth,
    )
}

/// A browser-shaped request: `Host` and `Origin` both present, plus the machine
/// bearer so a pass reaches the handler instead of stopping at the auth policy.
fn browser_get(host: &str, origin: &str) -> Request<Body> {
    Request::builder()
        .uri("/api/about")
        .header(header::HOST, host)
        .header(header::ORIGIN, origin)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .expect("a well-formed test request")
}

async fn status(bound: &str, req: Request<Body>) -> StatusCode {
    fresh_router(bound)
        .oneshot(req)
        .await
        .expect("the router must answer")
        .status()
}

#[tokio::test]
async fn the_gate_admits_the_bind_and_the_declared_name_and_nothing_else() {
    let dir = tempfile::tempdir().expect("a temp store");
    // Sole test in this file → no intra-process env race.
    std::env::set_var("RALPHY_DAEMON_DIR", dir.path());

    // The operator's browser, at the address the daemon actually bound.
    assert_eq!(
        status(
            BOUND,
            browser_get("100.64.0.1:7257", "http://100.64.0.1:7257")
        )
        .await,
        StatusCode::OK,
        "a browser at the bound address must reach the daemon"
    );

    // The same daemon reached by its declared name, behind something that
    // terminates TLS: https, and no port. Neither is visible to this listener.
    assert_eq!(
        status(BOUND, browser_get(DECLARED, &format!("https://{DECLARED}"))).await,
        StatusCode::OK,
        "a declared name must reach the daemon through a TLS terminator"
    );

    // Everything the gate exists to refuse, still refused.
    for (host, origin, why) in [
        (
            "100.64.0.1:7257",
            "https://evil.example",
            "a page the operator merely visits",
        ),
        (
            "100.64.0.1:7257",
            "null",
            "an opaque origin (sandboxed iframe, file://)",
        ),
        (
            "100.64.0.1:7257",
            "http://100.64.0.1:7258",
            "another app on a different port of the same address",
        ),
        (
            "rebind.evil.example",
            "http://100.64.0.1:7257",
            "an UNDECLARED name, however it resolves — the rebinding boundary",
        ),
    ] {
        assert_eq!(
            status(BOUND, browser_get(host, origin)).await,
            StatusCode::FORBIDDEN,
            "must be refused: {why}"
        );
    }

    // A REVERSE TUNNEL inverts the shape: the daemon stays on loopback and the
    // tunnel agent dials out to it, so `Host`/`Origin` carry the tunnel's public
    // hostname while the bind is `127.0.0.1`. Declaring the hostname is therefore
    // required even with no `--bind` at all — the tunnel is the common remote path,
    // so it gets its own coverage rather than riding on the network-bind case.
    assert_eq!(
        status(LOOPBACK, browser_get(TUNNEL, &format!("https://{TUNNEL}"))).await,
        StatusCode::OK,
        "a declared tunnel hostname must reach a loopback-bound daemon"
    );
    // Loopback itself keeps working alongside it — the operator is still local.
    assert_eq!(
        status(
            LOOPBACK,
            browser_get("127.0.0.1:7257", "http://127.0.0.1:7257")
        )
        .await,
        StatusCode::OK,
        "declaring a tunnel host must not evict the loopback spellings"
    );
    // Another tenant of the same tunnel domain is a different host and a different
    // origin. This is why the declaration is an exact name and never a wildcard.
    assert_eq!(
        status(
            LOOPBACK,
            browser_get(
                "someone-else-7257.use.devtunnels.ms",
                "https://someone-else-7257.use.devtunnels.ms"
            )
        )
        .await,
        StatusCode::FORBIDDEN,
        "a different tenant on the same tunnel domain must be refused"
    );

    std::env::remove_var("RALPHY_DAEMON_DIR");
}
