use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use tower::ServiceExt;

use crate::{auth, epoch, registry, router, totp, StorePaths};

/// A PNG of 64 bytes: the signature, then padding.
fn png() -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(64, 7);
    bytes
}

/// A store with the repo `demo` holding `a.png`, kept alive with the router.
struct Fixture {
    _dir: tempfile::TempDir,
    registry_path: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("a temp dir");
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).expect("the repo folder");
    std::fs::write(repo.join("a.png"), png()).expect("the image");
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("demo", &repo.to_string_lossy());
    registry::save_to(&store, &registry_path).expect("the registry is saved");
    Fixture {
        _dir: dir,
        registry_path,
    }
}

fn router_on(registry_path: &Path, auth: Arc<auth::AuthState>) -> Router {
    let (_tx, shutdown) = tokio::sync::watch::channel(false);
    router(
        None,
        registry_path.to_path_buf(),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        std::time::Instant::now(),
        shutdown,
        auth,
    )
}

const URI: &str = "/api/file/bytes?repo=demo&path=a.png";

async fn get(fx: &Fixture, headers: &[(&str, &str)]) -> Response {
    let mut req = Request::builder().uri(URI);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    router_on(&fx.registry_path, auth::AuthState::localhost())
        .oneshot(req.body(Body::empty()).expect("a request"))
        .await
        .expect("the router answers")
}

fn header_of(resp: &Response, name: header::HeaderName) -> Option<&str> {
    resp.headers().get(name).and_then(|v| v.to_str().ok())
}

#[tokio::test]
async fn a_range_request_answers_206() {
    let fx = fixture();
    let resp = get(&fx, &[("range", "bytes=0-9"), ("sec-fetch-dest", "empty")]).await;
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        header_of(&resp, header::CONTENT_RANGE),
        Some("bytes 0-9/64")
    );
    assert_eq!(header_of(&resp, header::ACCEPT_RANGES), Some("bytes"));
    assert_eq!(header_of(&resp, header::CONTENT_TYPE), Some("image/png"));
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("the body");
    assert_eq!(body.as_ref(), png().get(..10).expect("ten bytes"));
}

#[tokio::test]
async fn a_document_destination_is_refused() {
    let fx = fixture();
    for dest in ["document", "iframe", "object"] {
        let resp = get(&fx, &[("sec-fetch-dest", dest)]).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{dest}");
    }
    for dest in ["empty", "image", "video", "audio"] {
        let resp = get(&fx, &[("sec-fetch-dest", dest)]).await;
        assert_eq!(resp.status(), StatusCode::OK, "{dest}");
    }
}

#[tokio::test]
async fn an_answer_says_inline_and_no_store() {
    let fx = fixture();
    let cases: [&[(&str, &str)]; 3] = [
        &[],
        &[("range", "bytes=0-9")],
        &[("sec-fetch-dest", "document")],
    ];
    for headers in cases {
        let resp = get(&fx, headers).await;
        assert_eq!(
            header_of(&resp, header::CONTENT_DISPOSITION),
            Some("inline"),
            "{headers:?}"
        );
        assert_eq!(
            header_of(&resp, header::CACHE_CONTROL),
            Some("no-store"),
            "{headers:?}"
        );
    }
}

#[tokio::test]
async fn a_request_without_a_destination_is_served() {
    let fx = fixture();
    let resp = get(&fx, &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("the body");
    assert_eq!(body.as_ref(), png().as_slice());
}

#[tokio::test]
async fn a_request_without_a_session_is_refused() {
    let fx = fixture();
    let policy = auth::AuthPolicy::Session(Arc::new(auth::SessionAuth {
        token: "tok".to_owned(),
        totp: totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
        password: None,
        epoch: epoch::SessionEpoch::in_memory_detached(),
        revoked: auth::RevokedLogins::default(),
    }));
    let state = auth::AuthState::fixed(policy, epoch::SessionEpoch::in_memory_detached());
    let resp = router_on(&fx.registry_path, state)
        .oneshot(
            Request::builder()
                .uri(URI)
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("the router answers");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
