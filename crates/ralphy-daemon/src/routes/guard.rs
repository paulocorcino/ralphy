//! The auth guard over the whole axum surface (docs/adr/0032 §4 and
//! amendment): cross-site first, then the current policy, then the session
//! cookie.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::routes::now_unix;
use crate::{auth, cookie};

/// API endpoints reachable WITHOUT a session cookie under a `Session` policy —
/// the SPA's own login gate posts to these before it holds a cookie. Every other
/// `/api/*` and `/ws/*` endpoint stays gated; static UI bytes are served ungated
/// (see [`require_auth`]). `/api/logout` is NOT here (audit F5): it bumps the
/// session epoch for everyone, and a caller with no session has no one to log
/// off — allowlisted, it was an unauthenticated global invalidation.
pub(crate) const LOGIN_ALLOWLIST: &[&str] = &["/api/login", "/api/session"];

/// Whether the request reached the daemon over https — a TLS-terminating front
/// in between said so with `X-Forwarded-Proto: https` (ADR-0032 audit
/// amendment, F6; measured on dev tunnels 2026-09-21, which also forwards
/// `X-Forwarded-Host`/`X-Real-IP`). Trusting the header is sound because the
/// guard already limits who reaches the daemon to loopback and the declared
/// hosts, so a front is the only party that can set it; a local process that
/// forges it only earns itself a cookie its own plain-http browser will not
/// send back. The first value wins when the front chained several.
pub(crate) fn request_is_https(headers: &header::HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("https"))
}

/// The guard over the whole axum surface. First asks the [`auth::AuthPolicy`]
/// (`Localhost` passes all; `Bearer`, and the machine leg of `Session`, pass a
/// correct `Bearer <token>`). Under a `Session` policy a request with no valid
/// bearer is then checked for a browser session cookie; failing that, a top-level
/// `GET` navigation serves the static SPA (which renders its own login gate) and
/// anything else is `401`. `Localhost`/`Bearer` keep the plain `401`
/// fall-through — fail closed.
pub(crate) async fn require_auth(
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
            // `Secure` follows the request's own scheme, as at login — the two
            // must agree for one session.
            let secure = request_is_https(req.headers());
            let slid = session
                .slide_cookie(cookie_header, now)
                .map(|(c, kind)| cookie::set_cookie_value_with(&c, kind, secure));
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
