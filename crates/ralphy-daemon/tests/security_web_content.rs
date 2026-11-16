//! The web-content opt-in (ADR-0089, amendment of 2026-11-16): off by default,
//! the policy of a preview file names only the route; turning it on costs a
//! fresh code once a TOTP seed is armed, and the NEXT answer of the route
//! takes scripts, styles and fonts from the web. Drives the `Router`
//! in-process via `oneshot`, like `tests/security_remote_images.rs`.
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

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use ralphy_daemon::auth::{self, AuthState};
use ralphy_daemon::{registry, router};
use tower::ServiceExt;

/// The RFC 6238 test-vector seed, armed directly (a confirmed enrolment).
const SEED: &[u8] = b"12345678901234567890";
const HOST: &str = "127.0.0.1:7257";

/// One router per call (`oneshot` consumes it); one SHARED `AuthState` so the
/// flag the route re-reads is the one the preview route sees.
fn app(state: &Arc<AuthState>, store: &Path) -> axum::Router {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let _kept = Box::leak(Box::new(tx));
    router(
        None,
        store.join("repos.toml"),
        store.join("usage"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        Arc::clone(state),
    )
}

fn request(method: &str, path: &str, kind: &str, body: String) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, HOST)
        .header(header::CONTENT_TYPE, kind)
        .body(Body::from(body))
        .expect("build the request")
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("read the response body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("the body is JSON")
}

async fn set_web_content(state: &Arc<AuthState>, store: &Path, body: &str) -> StatusCode {
    let form = "application/x-www-form-urlencoded";
    app(state, store)
        .oneshot(request(
            "POST",
            "/api/security/web-content",
            form,
            body.into(),
        ))
        .await
        .expect("the router answers")
        .status()
}

/// The policy of a file of a fresh preview token, and what the state route says.
async fn policy_and_state(state: &Arc<AuthState>, store: &Path) -> (String, bool) {
    let command =
        serde_json::json!({ "id": 1, "verb": "file.preview", "payload": { "repo": "demo" } });
    let resp = app(state, store)
        .oneshot(request(
            "POST",
            "/api/command",
            "application/json",
            command.to_string(),
        ))
        .await
        .expect("the router answers");
    let reply = body_json(resp).await;
    let token = reply
        .get("token")
        .and_then(serde_json::Value::as_str)
        .expect("the verb gives a token");
    let file = app(state, store)
        .oneshot(request(
            "GET",
            &format!("/preview/{token}/page.html"),
            "text/plain",
            String::new(),
        ))
        .await
        .expect("the router answers");
    assert_eq!(file.status(), StatusCode::OK);
    let policy = file
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|v| v.to_str().ok())
        .expect("the answer carries a policy")
        .to_owned();
    let seen = app(state, store)
        .oneshot(request(
            "GET",
            "/api/security/state",
            "text/plain",
            String::new(),
        ))
        .await
        .expect("the router answers");
    let on = body_json(seen)
        .await
        .get("web_content")
        .and_then(serde_json::Value::as_bool)
        .expect("the state names the flag");
    (policy, on)
}

#[tokio::test]
async fn web_content_is_opt_in_and_costs_a_fresh_code_to_enable() {
    let dir = tempfile::tempdir().expect("a temp dir is writable");
    let store = dir.path();
    // Sole test in this file → no intra-process env race.
    std::env::set_var("RALPHY_DAEMON_DIR", store);
    let repo = tempfile::tempdir().expect("a temp dir is writable");
    std::fs::write(repo.path().join("page.html"), b"<p>page</p>").expect("the page is written");
    let mut repos = registry::RegistryStore::default();
    repos.upsert("demo", &repo.path().to_string_lossy());
    registry::save_to(&repos, &store.join("repos.toml")).expect("the registry is saved");
    let state = AuthState::localhost();

    let strict = |policy: &str| {
        policy.contains("/; style-src")
            && policy.contains("/; img-src")
            && policy.contains("/; connect-src 'none';")
    };
    let open = |policy: &str| {
        policy.contains("/ https:; style-src")
            && policy.contains("/ https:; img-src")
            && policy.contains("/ https:; connect-src 'none';")
    };

    // ── the default: off, and the policy names only the route ─────────────
    let (policy, on) = policy_and_state(&state, store).await;
    assert!(!on && strict(&policy), "{on} {policy}");

    // ── with no seed armed, enabling is free; the next answer is open ─────
    assert_eq!(
        set_web_content(&state, store, "enable=true").await,
        StatusCode::OK
    );
    assert!(auth::web_content_enabled_in(store));
    let (policy, on) = policy_and_state(&state, store).await;
    assert!(on && open(&policy), "{on} {policy}");

    // ── disabling is free, and the policy is strict again ─────────────────
    assert_eq!(
        set_web_content(&state, store, "enable=false").await,
        StatusCode::OK
    );
    let (policy, on) = policy_and_state(&state, store).await;
    assert!(!on && strict(&policy), "{on} {policy}");

    // ── with a seed armed, enabling without a code is refused ─────────────
    ralphy_daemon::totp::save_seed_to(
        &ralphy_daemon::totp::Seed::from_bytes(SEED.to_vec()),
        &ralphy_daemon::totp::seed_path_in(store),
    )
    .expect("the seed is saved");
    assert_eq!(
        set_web_content(&state, store, "enable=true").await,
        StatusCode::UNAUTHORIZED,
        "enable without a code"
    );
    assert!(!auth::web_content_enabled_in(store), "the flag stays off");
    let (policy, on) = policy_and_state(&state, store).await;
    assert!(!on && strict(&policy), "{on} {policy}");
}
