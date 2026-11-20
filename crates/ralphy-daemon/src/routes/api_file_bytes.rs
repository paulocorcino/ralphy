//! `GET /api/file/bytes`: the bytes of a project file, in ranges, for the
//! viewers of the workbench (ADR-0091 D2, D4, D5).

use std::path::PathBuf;

use axum::extract::Query;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use super::{blocking_read, RouterShared};
use crate::{checkout, confine, fleet, registry, tree};

/// The query: the repo, the path and the checkout, as `file.read` takes them.
#[derive(Deserialize)]
pub(crate) struct BytesQuery {
    repo: String,
    path: String,
    checkout: Option<String>,
}

/// The `Sec-Fetch-Dest` values the route serves: a fetch and a media element.
const SERVED_DESTINATIONS: &[&str] = &["empty", "image", "video", "audio"];

fn destination_refused(headers: &HeaderMap) -> bool {
    headers.get("sec-fetch-dest").is_some_and(|dest| {
        dest.to_str()
            .map_or(true, |dest| !SERVED_DESTINATIONS.contains(&dest))
    })
}

/// One `bytes=<start>-[<end>]` range. Any other form is ignored, as RFC 9110
/// lets a server ignore a `Range` it does not support.
fn parse_range(headers: &HeaderMap) -> Option<tree::ByteRange> {
    let value = headers.get(header::RANGE)?.to_str().ok()?;
    let (start, end) = value.trim().strip_prefix("bytes=")?.split_once('-')?;
    let start = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => None,
        end => Some(end.parse().ok()?),
    };
    Some(tree::ByteRange { start, end })
}

/// Where the route reads: the registry and this daemon's id.
#[derive(Clone)]
struct Local {
    registry_path: PathBuf,
    daemon_id: String,
}

/// The folder a local repo ref and checkout name. A peer ref, an unknown repo
/// or an unknown checkout is `None`.
fn local_root(local: &Local, query: &BytesQuery) -> Option<PathBuf> {
    let fleet::Route::Local { slug } = fleet::route(&query.repo, &local.daemon_id, &[]) else {
        return None;
    };
    let store = registry::load_from(&local.registry_path)
        .map_err(|e| tracing::warn!(error = %e, "repo registry unreadable for file bytes"))
        .ok()?;
    let repo = PathBuf::from(&store.entry_or_former(slug)?.path);
    let payload = serde_json::json!({ "checkout": query.checkout });
    let Some(named) = checkout::from_payload(&payload, &repo).ok()? else {
        return Some(repo);
    };
    let confined = named
        .confine(&repo)
        .map_err(|e| confine::miss((), "confining the checkout", &e))
        .ok()?;
    Some(confined.root().to_path_buf())
}

fn read(local: &Local, query: &BytesQuery, range: tree::ByteRange) -> Response {
    let Some(root) = local_root(local, query) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tree::read_range(&root, &query.path, range) {
        Ok(bytes) => ranged(bytes),
        Err(tree::BytesError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(tree::BytesError::TooLarge) => StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        Err(tree::BytesError::Unsatisfiable) => StatusCode::RANGE_NOT_SATISFIABLE.into_response(),
    }
}

/// `206` with `Content-Range` when the answer is part of the file, else `200`.
fn ranged(bytes: tree::Bytes) -> Response {
    let len = u64::try_from(bytes.bytes.len()).unwrap_or(u64::MAX);
    let whole = bytes.start == 0 && len == bytes.total;
    let content_range = (!whole).then(|| {
        let last = (bytes.start + len).saturating_sub(1);
        format!("bytes {}-{last}/{}", bytes.start, bytes.total)
    });
    let status = if whole {
        StatusCode::OK
    } else {
        StatusCode::PARTIAL_CONTENT
    };
    let mut resp = (status, bytes.bytes).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(bytes.media_type),
    );
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Some(value) = content_range.and_then(|v| HeaderValue::from_str(&v).ok()) {
        h.insert(header::CONTENT_RANGE, value);
    }
    resp
}

/// Every answer of the route, a refusal too, says `inline` and `no-store`.
fn with_answer_headers(mut resp: Response) -> Response {
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    resp
}

async fn serve(local: Local, query: BytesQuery, headers: HeaderMap) -> Response {
    if destination_refused(&headers) {
        return with_answer_headers(StatusCode::FORBIDDEN.into_response());
    }
    let range = parse_range(&headers).unwrap_or(tree::ByteRange {
        start: 0,
        end: None,
    });
    let answer = blocking_read(move || read(&local, &query, range))
        .await
        .unwrap_or_else(|| StatusCode::SERVICE_UNAVAILABLE.into_response());
    with_answer_headers(answer)
}

/// `/api/file/bytes`, behind the auth guard like every `/api` route.
pub(crate) fn file_bytes_routes(s: &RouterShared) -> Router {
    let local = Local {
        registry_path: s.registry_path.clone(),
        daemon_id: s.daemon_id.clone().unwrap_or_default(),
    };
    Router::new().route(
        "/api/file/bytes",
        get(move |Query(query): Query<BytesQuery>, headers: HeaderMap| {
            serve(local.clone(), query, headers)
        }),
    )
}

#[cfg(test)]
mod tests;
