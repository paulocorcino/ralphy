//! The embedded UI bytes: the fallback that serves every page and asset.

use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

use crate::{assets, UI};

/// Serve a file from the embedded UI tree. A page is served at its route
/// (`/`, `/popup`, `/fence`), never at its file name
/// ([`crate::assets::Shell`]). Every asset carries a content `ETag` under
/// `Cache-Control: no-cache`, so a reload is a round of `304`s, and a text
/// asset goes out gzipped when the browser admits it (see [`crate::assets`]).
pub(crate) async fn ui_asset(headers: HeaderMap, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let Some((path, file)) =
        assets::embedded_path(path).and_then(|p| UI.get_file(p).map(|f| (p, f)))
    else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let content_type = content_type(path);
    let contents = if path == assets::Shell::Desk.file() {
        assets::desk_page()
    } else {
        file.contents()
    };
    let prepared = assets::prepared(path, contents, assets::compressible(content_type)).await;
    let header_str = |name: header::HeaderName| headers.get(name).and_then(|v| v.to_str().ok());

    let mut resp = Response::builder()
        .header(header::ETAG, prepared.etag.as_str())
        .header(header::CACHE_CONTROL, assets::CACHE_CONTROL)
        .header(header::VARY, "Accept-Encoding");
    if assets::not_modified(header_str(header::IF_NONE_MATCH), &prepared.etag) {
        resp = resp.status(StatusCode::NOT_MODIFIED);
        return finish_asset(resp.body(axum::body::Body::empty()));
    }
    resp = resp.header(header::CONTENT_TYPE, content_type);
    let body = match &prepared.gzip {
        Some(gz) if assets::accepts_gzip(header_str(header::ACCEPT_ENCODING)) => {
            resp = resp.header(header::CONTENT_ENCODING, "gzip");
            axum::body::Body::from(gz.clone())
        }
        _ => axum::body::Body::from(contents),
    };
    finish_asset(resp.body(body))
}

/// The builder's only failure is an invalid header value, and every value
/// above is a literal or a hex string — a failure is a bug, answered with a
/// 500 rather than a panic on the request path.
fn finish_asset(built: Result<Response, axum::http::Error>) -> Response {
    match built {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!(error = %e, "building an asset response");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub(crate) fn content_type(path: &str) -> &'static str {
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
        // A manifest served as octet-stream is ignored by every browser, which
        // is a silent failure: the shell renders, "add to home screen" just
        // never offers a standalone launch.
        Some("webmanifest") => "application/manifest+json",
        _ => "application/octet-stream",
    }
}
