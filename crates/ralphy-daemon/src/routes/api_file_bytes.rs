//! `GET /api/file/bytes`: the bytes of a project file, in ranges, for the
//! viewers of the workbench (ADR-0091 D2, D4, D5).

use std::path::PathBuf;

use axum::extract::Query;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use super::{blocking_read, encode_query_value, RouterShared};
use crate::peer::client::{self, StreamDeadlines, STREAM_DEADLINES};
use crate::peer::{self, PeerDescriptor};
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

/// Where the route reads: the registry, the peer store and this daemon's id.
#[derive(Clone)]
struct Local {
    registry_path: PathBuf,
    peers_dir: PathBuf,
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
    let answer = match target(&local, &query.repo).await {
        Target::Local => read_off_reactor(local, query, &headers).await,
        Target::Peer(peer, slug) => {
            from_peer(&peer, &slug, &query, &headers, STREAM_DEADLINES).await
        }
        Target::Unknown => StatusCode::NOT_FOUND.into_response(),
    };
    with_answer_headers(answer)
}

async fn read_off_reactor(local: Local, query: BytesQuery, headers: &HeaderMap) -> Response {
    let range = parse_range(headers).unwrap_or(tree::ByteRange {
        start: 0,
        end: None,
    });
    blocking_read(move || read(&local, &query, range))
        .await
        .unwrap_or_else(|| StatusCode::SERVICE_UNAVAILABLE.into_response())
}

/// Where a repo ref is read.
enum Target {
    Local,
    Peer(Box<PeerDescriptor>, String),
    Unknown,
}

/// The peer store is read only for a ref this daemon does not own.
async fn target(local: &Local, repo: &str) -> Target {
    if !matches!(
        fleet::route(repo, &local.daemon_id, &[]),
        fleet::Route::UnknownDaemon { .. }
    ) {
        return Target::Local;
    }
    let (peers, _) = peer::read_peer_store(local.peers_dir.clone()).await;
    match fleet::route(repo, &local.daemon_id, &peers) {
        fleet::Route::Peer { peer, slug } => Target::Peer(Box::new(peer.clone()), slug.to_owned()),
        fleet::Route::Local { .. } | fleet::Route::UnknownDaemon { .. } => Target::Unknown,
    }
}

/// The peer's answer to the same request: its `Range`, then its status, its
/// headers of the content and its body as it arrives (D6).
async fn from_peer(
    peer: &PeerDescriptor,
    slug: &str,
    query: &BytesQuery,
    headers: &HeaderMap,
    deadlines: StreamDeadlines,
) -> Response {
    let path = peer_file_query(
        "/api/peer/file/bytes",
        slug,
        &query.path,
        query.checkout.as_deref(),
    );
    let mut forwarded = HeaderMap::new();
    if let Some(range) = headers.get(header::RANGE) {
        forwarded.insert(header::RANGE, range.clone());
    }
    match client::get_stream(peer, &path, forwarded, deadlines).await {
        Ok(resp) => passed_on(resp),
        Err(e) => {
            let why = client::transport_failed(peer, format!("{e:#}")).await;
            tracing::warn!(peer = %peer.daemon_id, %why, "the peer did not serve file bytes");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// The address of a peer route that reads one file of the peer's repo `slug`.
pub(crate) fn peer_file_query(
    route: &str,
    slug: &str,
    path: &str,
    checkout: Option<&str>,
) -> String {
    let mut query = format!(
        "{route}?repo={}&path={}",
        encode_query_value(slug),
        encode_query_value(path)
    );
    if let Some(name) = checkout {
        query.push_str("&checkout=");
        query.push_str(&encode_query_value(name));
    }
    query
}

/// The headers of the content a peer sent, on the peer's status and body.
fn passed_on(resp: Response) -> Response {
    let (parts, body) = resp.into_parts();
    let mut out = (parts.status, body).into_response();
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
    ] {
        if let Some(value) = parts.headers.get(&name) {
            out.headers_mut().insert(name, value.clone());
        }
    }
    out
}

/// `/api/peer/file/bytes`: a peer asks for the bytes of a repo of this
/// daemon's own registry, by its bare slug, so a request never routes again.
pub(crate) async fn serve_for_peer(
    registry_path: PathBuf,
    query: BytesQuery,
    headers: HeaderMap,
) -> Response {
    let local = Local {
        registry_path,
        peers_dir: PathBuf::new(),
        daemon_id: String::new(),
    };
    with_answer_headers(read_off_reactor(local, query, &headers).await)
}

/// `/api/peer/preview`: a peer asks for one file of the closed list of the
/// preview route (ADR-0089 D3), in a repo of this daemon's own registry. The
/// peer that asks writes the policy, because only it knows the browser's host.
pub(crate) async fn serve_preview_for_peer(registry_path: PathBuf, query: BytesQuery) -> Response {
    let local = Local {
        registry_path,
        peers_dir: PathBuf::new(),
        daemon_id: String::new(),
    };
    let read = blocking_read(move || {
        let root = local_root(&local, &query)?;
        crate::preview::read(&root, &query.path)
    });
    match read.await.flatten() {
        Some((media_type, bytes)) => {
            let mut resp = bytes.into_response();
            let h = resp.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            resp
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `/api/file/bytes`, behind the auth guard like every `/api` route.
pub(crate) fn file_bytes_routes(s: &RouterShared) -> Router {
    let local = Local {
        registry_path: s.registry_path.clone(),
        peers_dir: s.peers_dir.clone(),
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
pub(in crate::routes) mod peer_tests;
#[cfg(test)]
mod tests;
