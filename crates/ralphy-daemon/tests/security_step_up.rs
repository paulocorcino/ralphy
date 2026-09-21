//! Step-up on the operations that LOWER the auth posture (security audit
//! 2026-09-21, F2; ADR-0032 amendment E): with a live TOTP seed armed, `token/
//! remint`, `require-login enable=false` and `totp/revoke` demand a fresh code
//! with the login's anti-replay and throttle; with a password enrolled,
//! `/api/security/password` demands the current one and a body with no
//! `password` field never clears it. With nothing armed every one of them stays
//! frictionless (bootstrap). Drives the `Router` in-process via `oneshot`, like
//! `tests/security_routes.rs`.
//!
//! SOLE env-setter in its file: `RALPHY_DAEMON_DIR` is process-global, so this
//! env-setting test must be alone in its file (no intra-process race).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use ralphy_daemon::auth::AuthState;
use ralphy_daemon::router;
use sha1::Sha1;
use tower::ServiceExt;

/// The RFC 6238 test-vector seed, armed directly (a confirmed enrolment).
const SEED: &[u8] = b"12345678901234567890";

/// One router per call (`oneshot` consumes it); one SHARED `AuthState` so the
/// throttle and the last-consumed step persist across calls the way they do in
/// a running daemon. Localhost policy so `require_auth` passes and the
/// handlers run — the step-up is what is under test, not the gate.
fn app(state: &Arc<AuthState>) -> axum::Router {
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
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
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn post_form(path: &str, body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

/// The current RFC 6238 code for `SEED`, computed here rather than through the
/// crate (`Seed::code_at` is `pub(crate)` on purpose — the integration test
/// must not widen the public API to compute a code). `offset` steps of 30 s
/// away from now, for the replay case.
fn code(offset: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let counter = (now / 30) as i64 + offset;
    let mut mac = Hmac::<Sha1>::new_from_slice(SEED).unwrap();
    mac.update(&(counter as u64).to_be_bytes());
    let hs = mac.finalize().into_bytes();
    let off = (hs[hs.len() - 1] & 0x0f) as usize;
    let bin = ((hs[off] as u32 & 0x7f) << 24)
        | ((hs[off + 1] as u32) << 16)
        | ((hs[off + 2] as u32) << 8)
        | (hs[off + 3] as u32);
    format!("{:06}", bin % 1_000_000)
}

fn arm_seed(dir: &std::path::Path) {
    ralphy_daemon::totp::save_seed_to(
        &ralphy_daemon::totp::Seed::from_bytes(SEED.to_vec()),
        &ralphy_daemon::totp::seed_path_in(dir),
    )
    .unwrap();
}

fn seed_armed(dir: &std::path::Path) -> bool {
    ralphy_daemon::totp::load_seed_from(&ralphy_daemon::totp::seed_path_in(dir))
        .unwrap()
        .is_some()
}

/// The raw `daemon-password` record, or `None` when no password is enrolled.
/// Comparing records is how the test tells "the old hash stands" without
/// paying a PBKDF2 verify (1.3 M iterations, seconds each in a debug build).
fn password_record(dir: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(ralphy_daemon::password::password_path_in(dir)).ok()
}

/// Enrol `pw` directly as a `pbkdf2-sha1` record with a LOW iteration count.
/// The record format carries its own count and `verify` honours it (ADR-0032
/// amendment §D — that is what lets old hashes verify under their recorded
/// parameters), so the route's three verifies cost microseconds instead of
/// seconds. The route's own `hash_password` still runs at the real count.
fn enrol_cheap_password(dir: &std::path::Path, pw: &str) {
    let salt = [7u8; 16];
    let mut dk = [0u8; 20];
    pbkdf2::pbkdf2_hmac::<Sha1>(pw.as_bytes(), &salt, 1_000, &mut dk);
    let record = format!(
        "pbkdf2-sha1$1000${}${}",
        data_encoding::BASE64.encode(&salt),
        data_encoding::BASE64.encode(&dk)
    );
    std::fs::write(ralphy_daemon::password::password_path_in(dir), record).unwrap();
}

#[tokio::test]
async fn posture_downgrades_cost_a_fresh_factor_once_one_is_armed() {
    let dir = tempfile::tempdir().unwrap();
    // Sole test in this file → no intra-process env race.
    std::env::set_var("RALPHY_DAEMON_DIR", dir.path());
    let state = AuthState::localhost();

    // ── nothing armed: every downgrade is free (bootstrap) ────────────────
    let resp = app(&state)
        .oneshot(post_form("/api/security/token/remint", String::new()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "remint with no seed is free");
    let resp = app(&state)
        .oneshot(post_form("/api/security/totp/revoke", String::new()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "revoke with no seed is free");
    let resp = app(&state)
        .oneshot(post_form("/api/security/password", "password=first".into()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "first-time set is free");
    assert!(
        password_record(dir.path()).is_some(),
        "a record was written"
    );

    // ── password enrolled: changing or clearing costs the current one ─────
    enrol_cheap_password(dir.path(), "first");
    let enrolled = password_record(dir.path());
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/password",
            "password=second".into(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "change without `current` is refused"
    );
    assert_eq!(password_record(dir.path()), enrolled, "the old hash stands");
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/password",
            "password=second&current=wrong".into(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "wrong `current`");
    assert_eq!(password_record(dir.path()), enrolled);
    // A body with NO password field is the clear-by-omission the audit found:
    // it is a 400 now, and the factor survives it.
    let resp = app(&state)
        .oneshot(post_form("/api/security/password", "current=first".into()))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "absent field is not a clear"
    );
    assert_eq!(password_record(dir.path()), enrolled);
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/password",
            "password=second&current=first".into(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "right `current` changes it");
    let changed = password_record(dir.path());
    assert!(changed.is_some() && changed != enrolled, "a new record");
    // Cheap again for the clear's verify — same password, low count.
    enrol_cheap_password(dir.path(), "second");
    // An EMPTY password with the current one is the explicit clear.
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/password",
            "password=&current=second".into(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        body_string(resp).await.contains("\"password_set\":false"),
        "explicit clear with the current password"
    );
    assert_eq!(password_record(dir.path()), None, "the record is gone");

    // ── seed armed: remint / require-login off / revoke cost a code ───────
    // The require-login flag stays OFF on disk until the disable case: every
    // successful mutation rebuilds the live policy from disk (amendment §A),
    // and with the flag on that is the `Session` gate these cookie-less
    // requests would hit — the guard, not the step-up under test.
    arm_seed(dir.path());

    let resp = app(&state)
        .oneshot(post_form("/api/security/token/remint", String::new()))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "remint without a code"
    );
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/token/remint",
            "code=000000".into(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "remint with a wrong code"
    );
    let resp = app(&state)
        .oneshot(post_form("/api/security/totp/revoke", String::new()))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "revoke without a code"
    );
    assert!(seed_armed(dir.path()), "the seed survives");

    // A fresh code passes, and the SAME code is a replay right after.
    let fresh = code(0);
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/token/remint",
            format!("code={fresh}"),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "remint with the current code"
    );
    assert!(body_string(resp).await.contains("\"reminted\":true"));
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/totp/revoke",
            format!("code={fresh}"),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a consumed step is a replay (amendment §D), on step-up as on login"
    );
    assert!(seed_armed(dir.path()), "the seed survives the replay");

    // Lowering the gate: the flag goes on (disk only — no rebuild happened, the
    // live policy is still open), a code-less disable leaves it on, and the
    // NEXT step lifts it — the anti-replay is strictly-newer, not
    // one-per-window.
    ralphy_daemon::auth::set_require_login_in(dir.path(), true).unwrap();
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/require-login",
            "enable=false".into(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "disable without a code"
    );
    assert!(
        ralphy_daemon::auth::require_login_enabled_in(dir.path()),
        "the gate stays on"
    );
    let next = code(1);
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/require-login",
            format!("enable=false&code={next}"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "disable with a fresh code");
    assert!(!ralphy_daemon::auth::require_login_enabled_in(dir.path()));

    // ── the throttle: a stolen session cannot brute-force the six digits ──
    // A success resets the counter (the disable above did), so five fresh
    // failures reach the threshold and the next attempt — even with a valid
    // code — is 429 with Retry-After.
    for _ in 0..5 {
        let resp = app(&state)
            .oneshot(post_form("/api/security/totp/revoke", "code=000000".into()))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/totp/revoke",
            format!("code={}", code(2)),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS, "locked out");
    assert!(
        resp.headers().get(header::RETRY_AFTER).is_some(),
        "429 carries Retry-After"
    );
    assert!(seed_armed(dir.path()), "the seed survives the lockout");

    // Turning the gate ON never costs a code — not a downgrade, and it is how
    // the operator bootstraps — so the lockout does not touch it either. Last,
    // because it flips the live policy to `Session`.
    let resp = app(&state)
        .oneshot(post_form(
            "/api/security/require-login",
            "enable=true".into(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "enable stays free");
    assert!(ralphy_daemon::auth::require_login_enabled_in(dir.path()));

    std::env::remove_var("RALPHY_DAEMON_DIR");
}
