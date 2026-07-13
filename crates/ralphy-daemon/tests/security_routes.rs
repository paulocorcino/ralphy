//! The `/api/security/*` routes over the real store files (issue #195; ADR-0036
//! §2, ADR-0032 §4): `GET state` reports enrolment, `POST require-login` refuses
//! an enable with no seed (AC4), and `POST totp/enroll` mints a seed and flips the
//! reported state. Drives the `Router` in-process via `ServiceExt::oneshot` (no
//! socket).
//!
//! SOLE env-setter in its file: `RALPHY_DAEMON_DIR` is process-global, so this
//! env-setting test must be alone in its file (no intra-process race).

use std::path::PathBuf;
use std::time::Instant;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use ralphy_daemon::auth::AuthPolicy;
use ralphy_daemon::router;
use tower::ServiceExt;

/// A fresh router each call (`oneshot` consumes it), all reading the same
/// `RALPHY_DAEMON_DIR` for the security stores. Localhost policy so `require_auth`
/// passes and the handlers run.
fn fresh_router() -> axum::Router {
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        Instant::now(),
        rx,
        AuthPolicy::Localhost,
    )
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn get(path: &str) -> Request<Body> {
    Request::builder().uri(path).body(Body::empty()).unwrap()
}

fn post_form(path: &str, body: &'static str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn security_routes_reflect_and_gate_the_real_stores() {
    let dir = tempfile::tempdir().unwrap();
    // Sole test in this file → no intra-process env race.
    std::env::set_var("RALPHY_DAEMON_DIR", dir.path());

    // 1. Empty store → every factor unset.
    let resp = fresh_router()
        .oneshot(get("/api/security/state"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let s = body_string(resp).await;
    assert!(
        s.contains("\"totp_enrolled\":false") && s.contains("\"require_login\":false"),
        "empty store state: {s}"
    );

    // 2. AC4: enabling require-login with no seed is refused at the surface.
    let resp = fresh_router()
        .oneshot(post_form("/api/security/require-login", "enable=true"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "require-login without a seed must be a 400"
    );

    // 3. Enroll mints a seed and returns the real one-time provisioning URI.
    let resp = fresh_router()
        .oneshot(post_form("/api/security/totp/enroll", ""))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let s = body_string(resp).await;
    assert!(
        s.contains("otpauth://totp/ralphy:"),
        "enroll returns the real provisioning URI: {s}"
    );

    // 4. State now reports the enrolment (and the derived require_login).
    let resp = fresh_router()
        .oneshot(get("/api/security/state"))
        .await
        .unwrap();
    let s = body_string(resp).await;
    assert!(
        s.contains("\"totp_enrolled\":true") && s.contains("\"require_login\":true"),
        "after enroll: {s}"
    );

    // 5. With a seed present, enabling require-login is allowed.
    let resp = fresh_router()
        .oneshot(post_form("/api/security/require-login", "enable=true"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "enable with a seed is Ok");

    std::env::remove_var("RALPHY_DAEMON_DIR");
}
