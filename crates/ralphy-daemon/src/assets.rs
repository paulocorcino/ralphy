//! Serving the embedded workbench tree: a content-addressed `ETag` per asset,
//! the conditional GET that turns a reload into a `304`, and gzip for the text
//! assets — each computed once, lazily, the first time a path is asked for.
//!
//! The tree is `include_dir!`ed, so the URLs carry no content hash and can never
//! be cached `immutable`: a rebuilt binary serves the same `/app.js`. The
//! contract is therefore `Cache-Control: no-cache` — the browser keeps its copy
//! but ASKS every time — and the answer to that ask is a `304` with no body
//! whenever the bytes did not change. Correct after every rebuild, and the
//! 13 MB the shell loads (Monaco, Mermaid, the icon sets) crosses the wire once
//! per binary, gzipped.
//!
//! The DECISIONS are pure functions here and the handler (`routes::api_read::ui_asset`) only
//! sequences them; the byte work (a sha256 and a gzip of a 5 MB Monaco chunk)
//! runs on the blocking pool, never on the runtime.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};

/// What one embedded asset needs to be served: its validator, and its gzip
/// body when compressing it was worth it.
pub struct Prepared {
    /// A strong validator over the raw bytes, quoted (`"…"`), ready for the
    /// `ETag` header and for the `If-None-Match` compare.
    pub etag: String,
    /// The gzip body, or `None` when the type is not text or the compressed
    /// form was not smaller (a PNG, a WOFF2 — already compressed).
    pub gzip: Option<Bytes>,
}

/// The header value the browser must send back: revalidate on every use, keep
/// the copy meanwhile.
pub const CACHE_CONTROL: &str = "no-cache";

fn cache() -> &'static Mutex<HashMap<String, Arc<Prepared>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Prepared>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The prepared form of `path`, computed on first request. `compressible`
/// decides whether a gzip body is attempted at all; the caller derives it from
/// the content type so a binary asset costs a hash and nothing more.
pub async fn prepared(path: &str, contents: &'static [u8], compressible: bool) -> Arc<Prepared> {
    if let Some(hit) = lookup(path) {
        return hit;
    }
    // A miss races another miss for the same path at most once; both compute
    // the same value and the second insert is harmless.
    let computed = tokio::task::spawn_blocking(move || prepare(contents, compressible))
        .await
        // A panic on the blocking pool is a bug in `prepare`, not an I/O
        // outcome: hashing and deflating a static slice cannot fail.
        .expect("preparing an embedded asset never panics");
    let prepared = Arc::new(computed);
    match cache().lock() {
        Ok(mut map) => {
            map.insert(path.to_owned(), Arc::clone(&prepared));
        }
        Err(poisoned) => {
            poisoned
                .into_inner()
                .insert(path.to_owned(), Arc::clone(&prepared));
        }
    }
    prepared
}

fn lookup(path: &str) -> Option<Arc<Prepared>> {
    let map = match cache().lock() {
        Ok(map) => map,
        // A poisoned map still holds only fully-built entries: every insert is
        // a single `HashMap::insert` under the lock.
        Err(poisoned) => poisoned.into_inner(),
    };
    map.get(path).cloned()
}

/// The byte work, off the runtime.
fn prepare(contents: &[u8], compressible: bool) -> Prepared {
    let digest = Sha256::digest(contents);
    // Sixteen bytes of the digest: a collision needs 2^64 assets, and the tag
    // stays short enough to read in a devtools column.
    let mut etag = String::with_capacity(34);
    etag.push('"');
    for byte in &digest[..16] {
        etag.push_str(&format!("{byte:02x}"));
    }
    etag.push('"');

    let gzip = if compressible {
        let mut enc = GzEncoder::new(
            Vec::with_capacity(contents.len() / 3),
            Compression::default(),
        );
        // Writing a static slice into an in-memory encoder cannot fail; a
        // failure here would be a bug in flate2, so the asset falls back to
        // identity rather than taking the daemon down over a compression.
        let out = enc
            .write_all(contents)
            .and_then(|()| enc.finish())
            .unwrap_or_default();
        (!out.is_empty() && out.len() < contents.len()).then(|| Bytes::from(out))
    } else {
        None
    };
    Prepared { etag, gzip }
}

/// Whether an asset of this content type is worth deflating: text of every
/// kind, JS, JSON, SVG and the manifest. Fonts, PNGs and ICOs are stored
/// compressed already and would only cost the round trip.
pub fn compressible(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || content_type.starts_with("application/json")
        || content_type.starts_with("application/manifest+json")
        || content_type.starts_with("image/svg+xml")
}

/// Whether the request's `Accept-Encoding` admits gzip. A `gzip;q=0` is an
/// explicit refusal and is honoured; `*` admits it; a missing header does not.
pub fn accepts_gzip(accept_encoding: Option<&str>) -> bool {
    let Some(value) = accept_encoding else {
        return false;
    };
    value.split(',').any(|part| {
        let mut halves = part.trim().splitn(2, ';');
        let coding = halves.next().unwrap_or("").trim();
        if coding != "gzip" && coding != "*" {
            return false;
        }
        // `q=0` (and only an exact zero) is a refusal; an unparsable or absent
        // weight is the default 1.
        let weight = halves
            .next()
            .and_then(|params| {
                params
                    .split(';')
                    .filter_map(|p| p.trim().strip_prefix("q="))
                    .find_map(|q| q.trim().parse::<f32>().ok())
            })
            .unwrap_or(1.0);
        weight > 0.0
    })
}

/// Whether `If-None-Match` names this asset's current bytes: any tag in the
/// list, strong or weak-prefixed (`W/`), or the `*` wildcard.
pub fn not_modified(if_none_match: Option<&str>, etag: &str) -> bool {
    let Some(value) = if_none_match else {
        return false;
    };
    value.split(',').any(|tag| {
        let tag = tag.trim();
        tag == "*" || tag == etag || tag.strip_prefix("W/") == Some(etag)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_asset;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use axum::Router;
    use http_body_util::BodyExt;
    use std::io::Read;
    use tower::ServiceExt;

    #[test]
    fn text_types_compress_and_binary_types_do_not() {
        assert!(compressible("text/javascript; charset=utf-8"));
        assert!(compressible("text/css; charset=utf-8"));
        assert!(compressible("application/json"));
        assert!(compressible("application/manifest+json"));
        assert!(compressible("image/svg+xml"));
        assert!(!compressible("image/png"));
        assert!(!compressible("font/woff2"));
        assert!(!compressible("application/octet-stream"));
    }

    #[test]
    fn accept_encoding_is_read_the_way_browsers_write_it() {
        assert!(accepts_gzip(Some("gzip, deflate, br")));
        assert!(accepts_gzip(Some("br;q=1.0, gzip;q=0.8, *;q=0.1")));
        assert!(accepts_gzip(Some("*")));
        assert!(accepts_gzip(Some("gzip;q=0.5")));
        assert!(!accepts_gzip(Some("gzip;q=0")));
        assert!(!accepts_gzip(Some("gzip; q=0, identity")));
        assert!(!accepts_gzip(Some("br")));
        assert!(!accepts_gzip(Some("")));
        assert!(!accepts_gzip(None));
    }

    #[test]
    fn if_none_match_admits_strong_weak_lists_and_star() {
        let tag = "\"abc\"";
        assert!(not_modified(Some("\"abc\""), tag));
        assert!(not_modified(Some("W/\"abc\""), tag));
        assert!(not_modified(Some("\"zzz\", \"abc\""), tag));
        assert!(not_modified(Some("*"), tag));
        assert!(!not_modified(Some("\"zzz\""), tag));
        assert!(!not_modified(Some("abc"), tag));
        assert!(!not_modified(None, tag));
    }

    #[test]
    fn prepare_skips_gzip_when_it_would_not_shrink() {
        // Random-looking bytes deflate to MORE than themselves plus the header.
        let noise: Vec<u8> = (0..64u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
            .collect();
        let p = prepare(&noise, true);
        assert!(p.gzip.is_none(), "an incompressible body serves identity");
        assert_eq!(p.etag.len(), 34, "16 digest bytes as hex, quoted");
        let text = b"the same line over and over\n".repeat(64);
        let p = prepare(&text, true);
        assert!(p.gzip.is_some(), "repetitive text deflates");
        let p = prepare(&text, false);
        assert!(
            p.gzip.is_none(),
            "a non-compressible type is never deflated"
        );
    }

    fn router() -> Router {
        Router::new().fallback(ui_asset)
    }

    async fn get(path: &str, headers: &[(header::HeaderName, &str)]) -> axum::response::Response {
        let mut req = Request::builder().uri(path);
        for (name, value) in headers {
            req = req.header(name.clone(), *value);
        }
        router()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn header_str(resp: &axum::response::Response, name: header::HeaderName) -> Option<&str> {
        resp.headers().get(name).and_then(|v| v.to_str().ok())
    }

    #[tokio::test]
    async fn a_text_asset_is_gzipped_when_asked_and_round_trips() {
        let resp = get("/app.js", &[(header::ACCEPT_ENCODING, "gzip, deflate, br")]).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(header_str(&resp, header::CONTENT_ENCODING), Some("gzip"));
        assert_eq!(
            header_str(&resp, header::CACHE_CONTROL),
            Some(CACHE_CONTROL)
        );
        assert_eq!(header_str(&resp, header::VARY), Some("Accept-Encoding"));
        assert_eq!(
            header_str(&resp, header::CONTENT_TYPE),
            Some("text/javascript; charset=utf-8")
        );
        let etag = header_str(&resp, header::ETAG).map(str::to_owned);
        assert!(matches!(etag.as_deref(), Some(t) if t.starts_with('"') && t.ends_with('"')));

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let source = include_bytes!("../assets/ui/app.js");
        assert!(
            body.len() < source.len() / 2,
            "gzip halves the source at least"
        );
        let mut inflated = Vec::new();
        flate2::read::GzDecoder::new(&body[..])
            .read_to_end(&mut inflated)
            .unwrap();
        assert_eq!(
            inflated, source,
            "the gzip body inflates to the embedded bytes"
        );
    }

    #[tokio::test]
    async fn without_accept_encoding_the_asset_is_served_as_is_with_an_etag() {
        let resp = get("/app.js", &[]).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get(header::CONTENT_ENCODING).is_none());
        assert!(resp.headers().get(header::ETAG).is_some());
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], &include_bytes!("../assets/ui/app.js")[..]);
    }

    #[tokio::test]
    async fn a_matching_if_none_match_is_a_bodiless_304() {
        let first = get("/wb-changes.js", &[]).await;
        let etag = header_str(&first, header::ETAG).unwrap().to_owned();
        let resp = get(
            "/wb-changes.js",
            &[
                (header::IF_NONE_MATCH, etag.as_str()),
                (header::ACCEPT_ENCODING, "gzip"),
            ],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(header_str(&resp, header::ETAG), Some(etag.as_str()));
        assert_eq!(
            header_str(&resp, header::CACHE_CONTROL),
            Some(CACHE_CONTROL)
        );
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(body.is_empty(), "a 304 carries no body");

        let resp = get("/wb-changes.js", &[(header::IF_NONE_MATCH, "\"stale\"")]).await;
        assert_eq!(resp.status(), StatusCode::OK, "a stale tag gets the bytes");
    }

    #[tokio::test]
    async fn a_binary_asset_is_never_gzipped() {
        let resp = get("/icon-192.png", &[(header::ACCEPT_ENCODING, "gzip")]).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get(header::CONTENT_ENCODING).is_none());
        assert!(resp.headers().get(header::ETAG).is_some());
        assert_eq!(header_str(&resp, header::CONTENT_TYPE), Some("image/png"));
    }

    #[tokio::test]
    async fn a_missing_path_is_still_a_404() {
        let resp = get("/no-such-asset.js", &[(header::ACCEPT_ENCODING, "gzip")]).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
