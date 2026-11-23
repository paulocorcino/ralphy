//! `GET /preview/<token>/<path>`: the files of an HTML document whose scripts
//! the operator allowed (ADR-0089). The token is the only key, and every
//! answer carries the policy of [`preview::policy`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use super::guard::request_is_https;
use super::headers::OwnPolicy;
use super::{blocking_read, peer_file_query, RouterShared};
use crate::peer::client::{self, STREAM_DEADLINES};
use crate::peer::PeerDescriptor;
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
fn answer(media_type: HeaderValue, status: StatusCode, body: Body, policy: &str) -> Response {
    let Ok(policy) = HeaderValue::from_str(policy) else {
        return not_found();
    };
    let mut resp = (status, body).into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, media_type);
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

/// The media type and body of a file of the token, or `None` for a refusal.
type Served = Option<(HeaderValue, Body)>;

async fn read_local(root: PathBuf, rel: String) -> Served {
    let (media_type, bytes) = blocking_read(move || preview::read(&root, &rel))
        .await
        .flatten()?;
    Some((HeaderValue::from_static(media_type), Body::from(bytes)))
}

/// The file from the peer's own route, streamed (ADR-0089, amendment
/// 2026-11-23). The peer checks the closed list and confines the path.
async fn read_peer(peer: &PeerDescriptor, slug: &str, checkout: Option<&str>, rel: &str) -> Served {
    let path = peer_file_query("/api/peer/preview", slug, rel, checkout);
    let resp = match client::get_stream(peer, &path, HeaderMap::new(), STREAM_DEADLINES).await {
        Ok(resp) => resp,
        Err(e) => {
            let why = client::transport_failed(peer, format!("{e:#}")).await;
            tracing::warn!(peer = %peer.daemon_id, %why, "the peer did not serve a preview file");
            return None;
        }
    };
    if resp.status() != StatusCode::OK {
        return None;
    }
    let media_type = resp.headers().get(header::CONTENT_TYPE)?.clone();
    Some((media_type, resp.into_body()))
}

async fn serve(
    auth: Arc<auth::AuthState>,
    token: String,
    rel: String,
    headers: HeaderMap,
) -> Response {
    let Some(scope) = preview::PREVIEWS.scope_of(&token, Instant::now()) else {
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
    let served = match scope {
        preview::Scope::Local(root) => read_local(root, rel).await,
        preview::Scope::Peer {
            peer,
            slug,
            checkout,
        } => read_peer(&peer, &slug, checkout.as_deref(), &rel).await,
    };
    match served {
        Some((media_type, body)) => answer(media_type, StatusCode::OK, body, &policy),
        None => answer(
            HeaderValue::from_static("text/plain"),
            StatusCode::NOT_FOUND,
            Body::empty(),
            &policy,
        ),
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

#[cfg(test)]
mod peer_tests;
