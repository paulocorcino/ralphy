//! `GET /preview/<token>/<path>`: the files of an HTML document whose scripts
//! the operator allowed (ADR-0089). The token is the only key, and every
//! answer carries the policy of [`preview::policy`].

use std::sync::Arc;
use std::time::Instant;

use axum::extract::Path;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use super::guard::request_is_https;
use super::headers::OwnPolicy;
use super::{blocking_read, RouterShared};
use crate::{auth, preview};

/// The host as the browser names it. A front that rewrites `Host` says the
/// real one in `X-Forwarded-Host` (measured on dev tunnels, 2026-09-21).
fn browser_host(headers: &HeaderMap) -> Option<&str> {
    let named = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let host = named("x-forwarded-host")
        .and_then(|v| v.split(',').next())
        .or_else(|| named("host"))?
        .trim();
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']');
    (!host.is_empty() && host.chars().all(plain)).then_some(host)
}

fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

/// An answer that brings its own policy in place of the page's.
fn answer(media_type: &'static str, status: StatusCode, body: Vec<u8>, policy: &str) -> Response {
    let Ok(policy) = HeaderValue::from_str(policy) else {
        return not_found();
    };
    let mut resp = (status, body).into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
    h.insert(header::CONTENT_SECURITY_POLICY, policy);
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // A document with an opaque origin loads a font and a module only with
    // this header (ADR-0089, amendment of 2026-11-15). The token stays the key.
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp.extensions_mut().insert(OwnPolicy);
    resp
}

async fn serve(
    auth: Arc<auth::AuthState>,
    token: String,
    rel: String,
    headers: HeaderMap,
) -> Response {
    let Some(root) = preview::PREVIEWS.root_of(&token, Instant::now()) else {
        return not_found();
    };
    let Some(host) = browser_host(&headers) else {
        return not_found();
    };
    let scheme = if request_is_https(&headers) {
        "https"
    } else {
        "http"
    };
    let prefix = format!("{scheme}://{host}/preview/{token}/");
    let web = preview::Web {
        images: auth.remote_images(),
        content: auth.web_content(),
    };
    let policy = preview::policy(&prefix, web);
    match blocking_read(move || preview::read(&root, &rel))
        .await
        .flatten()
    {
        Some((media_type, bytes)) => answer(media_type, StatusCode::OK, bytes, &policy),
        None => answer("text/plain", StatusCode::NOT_FOUND, Vec::new(), &policy),
    }
}

/// `/preview/*`: reads only, and nothing here reads a cookie or a bearer.
pub(crate) fn preview_routes(s: &RouterShared) -> Router {
    let auth = Arc::clone(&s.auth);
    Router::new().route(
        "/preview/{token}/{*path}",
        get(
            move |Path((token, rel)): Path<(String, String)>, headers: HeaderMap| {
                serve(Arc::clone(&auth), token, rel, headers)
            },
        ),
    )
}
