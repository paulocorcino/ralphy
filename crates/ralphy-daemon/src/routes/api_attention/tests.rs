use std::path::PathBuf;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

use super::DISMISS_MAX_BYTES;
use crate::{auth, router, StorePaths};

/// The status of `POST /api/attention/dismiss` with `body`, on a router with
/// no peers and no console.
async fn dismiss_status(body: String) -> u16 {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_tx, shutdown) = tokio::sync::watch::channel(false);
    let response = router(
        None,
        dir.path().join("repos.toml"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        std::time::Instant::now(),
        shutdown,
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/attention/dismiss")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .expect("a request"),
    )
    .await
    .expect("the router answers");
    response.status().as_u16()
}

#[tokio::test]
async fn a_dismiss_body_over_the_cap_is_refused() {
    let id = |len: usize| format!(r#"{{"id":"{}"}}"#, "x".repeat(len));
    let fits = DISMISS_MAX_BYTES - id(0).len();
    assert_eq!(
        dismiss_status(id(fits)).await,
        404,
        "a body at the cap is read: an id that names no request"
    );
    assert_eq!(dismiss_status(id(fits + 1)).await, 413);
}
