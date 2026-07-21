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
use ralphy_daemon::auth::AuthState;
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
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        Instant::now(),
        rx,
        AuthState::localhost(),
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

    // 3. Enroll mints a PENDING seed and returns the real one-time provisioning
    //    URI — but the factor is NOT armed yet (ADR-0032 amendment §C).
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

    // 4. A pending seed does not count as enrolled and does not gate login: state
    //    stays false and require-login is still refused (AC4 holds).
    let resp = fresh_router()
        .oneshot(get("/api/security/state"))
        .await
        .unwrap();
    let s = body_string(resp).await;
    assert!(
        s.contains("\"totp_enrolled\":false") && s.contains("\"require_login\":false"),
        "a pending enrolment must not arm the factor: {s}"
    );
    let resp = fresh_router()
        .oneshot(post_form("/api/security/require-login", "enable=true"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "require-login still refused while enrolment is only pending"
    );

    // 5. A wrong confirm code leaves the factor unarmed (confirmed:false, not an
    //    error).
    let resp = fresh_router()
        .oneshot(post_form("/api/security/totp/confirm", "code=000000"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        body_string(resp).await.contains("\"confirmed\":false"),
        "a wrong code does not confirm"
    );

    // 6. Arm the live seed directly (a confirmed enrolment): totp is enrolled, but
    //    require_login is the PERSISTED flag — still off until the operator opts in.
    ralphy_daemon::totp::save_seed_to(
        &ralphy_daemon::totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
        &ralphy_daemon::totp::seed_path_in(dir.path()),
    )
    .unwrap();
    let resp = fresh_router()
        .oneshot(get("/api/security/state"))
        .await
        .unwrap();
    let s = body_string(resp).await;
    assert!(
        s.contains("\"totp_enrolled\":true") && s.contains("\"require_login\":false"),
        "armed seed alone does not require login (flag is opt-in): {s}"
    );

    // 7. With a seed armed, enabling require-login is allowed and persists the flag.
    let resp = fresh_router()
        .oneshot(post_form("/api/security/require-login", "enable=true"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "enable with a seed is Ok");
    let resp = fresh_router()
        .oneshot(get("/api/security/state"))
        .await
        .unwrap();
    assert!(
        body_string(resp).await.contains("\"require_login\":true"),
        "require-login flag now reads back on"
    );

    // 8. The live runtime swap (amendment §A): one ROUTER instance (one shared
    //    AuthState) starts on localhost (open), and enabling require-login through
    //    it gates the very next request — no restart. Disable the flag first so the
    //    router boots open.
    ralphy_daemon::auth::set_require_login_in(dir.path(), false).unwrap();
    let app = fresh_router();
    // Before: a gated route is authorized without any credential (Localhost).
    let resp = app
        .clone()
        .oneshot(get("/api/security/state"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "loopback is open before the gate"
    );
    // Enable require-login through the SAME router → its policy swaps to Session.
    let resp = app
        .clone()
        .oneshot(post_form("/api/security/require-login", "enable=true"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // After: the same gated route now demands a login (no cookie/bearer → 401).
    let resp = app
        .clone()
        .oneshot(get("/api/security/state"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "the gate engaged immediately for the next request"
    );

    std::env::remove_var("RALPHY_DAEMON_DIR");
}
