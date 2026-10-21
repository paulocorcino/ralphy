//! The bearer policy and the login flow of the session policy.

use super::*;

/// The bearer gate on a data route: a missing or wrong token is refused
/// before the handler runs, the right token is served, and the localhost
/// policy needs no token at all.
#[tokio::test]
async fn the_bearer_policy_admits_only_the_right_token() {
    let bearer = || {
        auth::AuthState::fixed(
            auth::AuthPolicy::Bearer("tok".into()),
            epoch::SessionEpoch::in_memory_detached(),
        )
    };
    // (case, auth state, Authorization header, expected status)
    let rows = [
        (
            "bearer, no header",
            bearer(),
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            "bearer, wrong token",
            bearer(),
            Some("Bearer wrong"),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "bearer, right token",
            bearer(),
            Some("Bearer tok"),
            StatusCode::OK,
        ),
        (
            "localhost, no header",
            auth::AuthState::localhost(),
            None,
            StatusCode::OK,
        ),
    ];
    for (case, state, authorization, want) in rows {
        let id = identity::Identity {
            id: ulid::Ulid::nil(),
            name: "anvil".into(),
            avatar: "🐙".into(),
        };
        let mut request = Request::builder().uri("/api/identity");
        if let Some(value) = authorization {
            request = request.header(header::AUTHORIZATION, value);
        }
        let resp = router(
            Some(id),
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            state,
        )
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
        assert_eq!(resp.status(), want, "{case}");
    }
}

/// The full browser-login round trip under a `Session` policy (issue #179,
/// promoted in #200): no-cookie `401` on data, the SPA shell is served ungated
/// at `/` (it renders its own login gate), a valid-TOTP `POST /api/login` `200`
/// + `Set-Cookie`, the cookie authorizes a follow-up, and a machine `Bearer`
/// still authorizes. Plumbing only — the code itself is pinned by the `totp`
/// RFC-vector unit test.
#[tokio::test]
async fn session_policy_login_flow() {
    // 1. No cookie / no bearer → every DATA endpoint is 401.
    for uri in [
        "/api/identity",
        "/ws/session?repo=x&agent=claude",
        "/ws/command",
    ] {
        let resp = session_router("tok")
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "{uri} must be 401 with no cookie"
        );
    }

    // 2. The shell (which hosts its own login gate) is served without a
    // cookie, NOT redirected.
    let resp = session_router("tok")
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET / → 200 (ungated shell)");

    // 3. A valid current TOTP mints a session cookie.
    let now = now_unix();
    let code = rfc_seed().code_at(now / 30);
    let resp = session_router("tok")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("code={code}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "valid TOTP → 200");
    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("a Set-Cookie header")
        .to_string();
    assert!(
        set_cookie.contains("ralphy_session="),
        "cookie name: {set_cookie}"
    );
    assert!(set_cookie.contains("HttpOnly"), "HttpOnly: {set_cookie}");
    assert!(
        set_cookie.contains("SameSite=Strict"),
        "SameSite: {set_cookie}"
    );

    // The cookie value is everything up to the first attribute `;`.
    let cookie_pair = set_cookie.split(';').next().unwrap().to_string();

    // 4. That cookie authorizes a follow-up request.
    let resp = session_router("tok")
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .header(header::COOKIE, &cookie_pair)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "cookie authorizes → 200");

    // 5. The machine path is unchanged: a correct Bearer still authorizes.
    let resp = session_router("tok")
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .header(header::AUTHORIZATION, "Bearer tok")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "Bearer authorizes under Session"
    );
}

#[tokio::test]
async fn remember_me_mints_a_week_long_cookie_that_authorizes() {
    // ADR-0032 amendment 2026-09-16: the box is opt-in — absent, the cookie
    // carries the standard 30 min Max-Age; `remember=true` the 7-day one.
    let standard = login_set_cookie("").await;
    assert!(
        standard.contains("Max-Age=1800"),
        "no box → standard idle: {standard}"
    );
    let remembered = login_set_cookie("&remember=true").await;
    assert!(
        remembered.contains("Max-Age=604800"),
        "remember=true → 7-day idle: {remembered}"
    );
    let cookie_pair = remembered.split(';').next().unwrap().to_string();
    assert!(
        cookie_pair.contains(".r."),
        "the value carries the remembered kind: {cookie_pair}"
    );
    let resp = session_router("tok")
        .oneshot(
            Request::builder()
                .uri("/api/identity")
                .header(header::COOKIE, &cookie_pair)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a remembered cookie authorizes → 200"
    );
}

/// `GET /api/session` reports the policy and whether the caller is authed. It
/// is allowlisted: a session caller with no cookie gets 200 with `authed`
/// false, and localhost is always authed.
#[tokio::test]
async fn api_session_reports_the_policy_and_whether_authed() {
    let get_session = |router: axum::Router, header: Option<(header::HeaderName, String)>| async move {
        let mut request = Request::builder().uri("/api/session");
        if let Some((name, value)) = header {
            request = request.header(name, value);
        }
        let resp = router
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "allowlisted: {resp:?}");
        body_string(resp).await
    };

    // A valid minted cookie.
    let code = rfc_seed().code_at(now_unix() / 30);
    let login = session_router("tok")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("code={code}")))
                .unwrap(),
        )
        .await
        .unwrap();
    let set_cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("a Set-Cookie header")
        .to_string();
    let cookie_pair = set_cookie.split(';').next().unwrap().to_string();
    let bearer = router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::fixed(
            auth::AuthPolicy::Bearer("tok".into()),
            epoch::SessionEpoch::in_memory_detached(),
        ),
    );

    // (case, body, policy, authed)
    let rows = [
        (
            "localhost",
            body_string(get_local("/api/session").await).await,
            "localhost",
            true,
        ),
        (
            "session, no cookie",
            get_session(session_router("tok"), None).await,
            "session",
            false,
        ),
        (
            "session, valid cookie",
            get_session(session_router("tok"), Some((header::COOKIE, cookie_pair))).await,
            "session",
            true,
        ),
        (
            "bearer, matching header",
            get_session(bearer, Some((header::AUTHORIZATION, "Bearer tok".into()))).await,
            "bearer",
            true,
        ),
    ];
    for (case, body, policy, authed) in rows {
        assert!(
            body.contains(&format!(r#""policy":"{policy}""#)),
            "{case}: policy: {body}"
        );
        assert!(
            body.contains(&format!(r#""authed":{authed}"#)),
            "{case}: authed: {body}"
        );
    }
}
