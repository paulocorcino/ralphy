//! The remote-images opt-in (ADR-0032 amendment §F): off by default, the CSP
//! `img-src` has no `https:`; turning it on costs a fresh code once a TOTP seed
//! is armed, and the NEXT response carries the permissive policy; turning it
//! off is free and the policy is strict again. Drives the `Router` in-process
//! via `oneshot`, like `tests/security_step_up.rs`.
//!
//! SOLE env-setter in its file: `RALPHY_DAEMON_DIR` is process-global, so this
//! env-setting test must be alone in its file (no intra-process race).
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]
#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use ralphy_daemon::auth::{self, AuthState};
use ralphy_daemon::router;
use sha1::Sha1;
use tower::ServiceExt;

/// The RFC 6238 test-vector seed, armed directly (a confirmed enrolment).
const SEED: &[u8] = b"12345678901234567890";

const STRICT_IMG: &str = "img-src 'self' data: blob:;";
const REMOTE_IMG: &str = "img-src 'self' data: blob: https:;";

/// One router per call (`oneshot` consumes it); one SHARED `AuthState` so the
/// flag the route re-reads is the one the header layer sees.
fn app(state: &Arc<AuthState>) -> axum::Router {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let _kept = Box::leak(Box::new(tx));
    router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        Arc::clone(state),
    )
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("read the response body")
        .to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("build the request")
}

fn post_form(path: &str, body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .expect("build the request")
}

/// The CSP a plain page load receives now.
async fn csp(state: &Arc<AuthState>) -> String {
    let resp = app(state).oneshot(get("/")).await.unwrap();
    resp.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .expect("read the CSP header as text")
        .to_owned()
}

/// The current RFC 6238 code for `SEED`, computed here because `Seed::code_at`
/// is `pub(crate)` on purpose.
fn code() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("read the system clock")
        .as_secs();
    let mut mac = Hmac::<Sha1>::new_from_slice(SEED).expect("key the HMAC");
    mac.update(&(now / 30).to_be_bytes());
    let hs = mac.finalize().into_bytes();
    let off = usize::from(hs.last().expect("a SHA-1 digest is 20 bytes") & 0x0f);
    let [b0, b1, b2, b3] = *hs
        .get(off..)
        .and_then(<[u8]>::first_chunk::<4>)
        .expect("a SHA-1 digest is 20 bytes and the offset is at most 15");
    let bin = ((u32::from(b0) & 0x7f) << 24)
        | (u32::from(b1) << 16)
        | (u32::from(b2) << 8)
        | u32::from(b3);
    format!("{:06}", bin % 1_000_000)
}

#[tokio::test]
async fn remote_images_is_opt_in_and_costs_a_fresh_code_to_enable() {
    let dir = tempfile::tempdir().unwrap();
    // Sole test in this file → no intra-process env race.
    std::env::set_var("RALPHY_DAEMON_DIR", dir.path());
    let state = AuthState::localhost();

    // ── the default: off, and the policy admits no remote image ───────────
    let resp = app(&state)
        .oneshot(get("/api/security/state"))
        .await
        .unwrap();
    assert!(body_string(resp).await.contains("\"remote_images\":false"));
    let policy = csp(&state).await;
    assert!(policy.contains(STRICT_IMG), "{policy}");

    // ── enabling with a seed armed costs a code ───────────────────────────
    ralphy_daemon::totp::save_seed_to(
        &ralphy_daemon::totp::Seed::from_bytes(SEED.to_vec()),
        &ralphy_daemon::totp::seed_path_in(dir.path()),
    )
    .unwrap();
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/remote-images",
            "enable=true".into(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "enable without a code"
    );
    assert!(
        !auth::remote_images_enabled_in(dir.path()),
        "the flag stays off"
    );
    assert!(csp(&state).await.contains(STRICT_IMG));

    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/remote-images",
            format!("enable=true&code={}", code()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "enable with a fresh code");
    assert!(auth::remote_images_enabled_in(dir.path()));
    let resp = app(&state)
        .oneshot(get("/api/security/state"))
        .await
        .unwrap();
    assert!(body_string(resp).await.contains("\"remote_images\":true"));
    let policy = csp(&state).await;
    assert!(policy.contains(REMOTE_IMG), "the next response: {policy}");

    // ── disabling is free, and the policy is strict again ─────────────────
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/remote-images",
            "enable=false".into(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "disable needs no code");
    assert!(!auth::remote_images_enabled_in(dir.path()));
    let policy = csp(&state).await;
    assert!(policy.contains(STRICT_IMG), "{policy}");
}
