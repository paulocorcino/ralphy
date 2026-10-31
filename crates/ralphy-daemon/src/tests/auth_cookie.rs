//! The session cookie: its scheme, its logout, its refusals, and the remote-exec routes.

use super::*;

/// `Secure` follows the request's scheme (audit F6, layer 2): a login that
/// came through a TLS front (`X-Forwarded-Proto: https`, which dev tunnels
/// forwards — measured 2026-09-21) mints a `Secure` cookie; a plain-http
/// login does not; the idle-slide re-issue and the clear on logout carry the
/// same answer, because a slide that dropped the attribute would silently
/// downgrade the session and one that added it would strand a plain-http
/// browser.
#[tokio::test]
async fn secure_cookie_follows_the_forwarded_scheme_on_login_slide_and_logout() {
    let now = now_unix();
    let code = rfc_seed().code_at(now / 30);
    let login = |forwarded: Option<&'static str>| {
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(proto) = forwarded {
            req = req.header("x-forwarded-proto", proto);
        }
        req.body(Body::from(format!("code={code}"))).unwrap()
    };
    let set_cookie = |resp: &Response| {
        resp.headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("a Set-Cookie header")
            .to_string()
    };

    // Plain http: no `Secure` (the loopback default).
    let resp = session_router("tok").oneshot(login(None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        !set_cookie(&resp).contains("Secure"),
        "{}",
        set_cookie(&resp)
    );

    // Through a TLS front: `Secure`. (Different routers share the token and
    // the detached epoch's start value, and a step already consumed on one
    // is not consumed on another — each has its own last-step store.)
    let resp = session_router("tok")
        .oneshot(login(Some("https")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        set_cookie(&resp).ends_with("; Secure"),
        "{}",
        set_cookie(&resp)
    );
    // A front that says http, or a chain whose FIRST hop is http, is not https.
    let resp = session_router("tok")
        .oneshot(login(Some("http, https")))
        .await
        .unwrap();
    assert!(
        !set_cookie(&resp).contains("Secure"),
        "{}",
        set_cookie(&resp)
    );

    // The idle-slide re-issue: a cookie far enough into its idle window that
    // the guard re-issues it on this request, once over https and once over
    // http, on the SAME router (the slide signs under the router's epoch).
    let app = session_router("tok");
    let stale = cookie::sign(
        "tok",
        0,
        cookie::SessionKind::Standard,
        now - cookie::SLIDE_MIN_SECS - 10,
        now + cookie::SESSION_IDLE_TTL_SECS - cookie::SLIDE_MIN_SECS - 10,
        cookie::LoginId::mint(),
    );
    for (forwarded, want_secure) in [(Some("https"), true), (None, false)] {
        let mut req = Request::builder()
            .uri("/api/identity")
            .header(header::COOKIE, format!("ralphy_session={stale}"));
        if let Some(proto) = forwarded {
            req = req.header("x-forwarded-proto", proto);
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the stale cookie still authorizes"
        );
        let slid = set_cookie(&resp);
        assert_eq!(
            slid.contains("Secure"),
            want_secure,
            "slide over {forwarded:?}: {slid}"
        );
    }

    // Logout: the clear carries the attribute of the request that asked.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/logout")
                .header(header::COOKIE, format!("ralphy_session={stale}"))
                .header("x-forwarded-proto", "https")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cleared = set_cookie(&resp);
    assert!(
        cleared.contains("Max-Age=0") && cleared.ends_with("; Secure"),
        "{cleared}"
    );
}

/// Logging off takes a session (audit F5): the route bumps the epoch for
/// EVERY cookie, so a caller without one — who has nobody to log off — is
/// refused before the bump, and the cookies that were valid stay valid.
/// With a cookie it clears it server-side as before.
#[tokio::test]
async fn logout_clears_cookie_and_needs_a_session() {
    let cookie = login_set_cookie("").await;
    let cookie_pair = cookie.split(';').next().unwrap().to_string();
    // ONE router: the epoch lives in its `AuthState`, and the claim is that
    // an unauthenticated POST did not move it.
    let app = session_router("tok");

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/logout")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "no session → nobody to log off"
    );
    let resp = app
        .clone()
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
        "the live cookie survived the refused log-off"
    );

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/logout")
                .header(header::COOKIE, &cookie_pair)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "POST /api/logout → 200");
    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("a Set-Cookie header");
    assert!(
        set_cookie.contains("ralphy_session=;") && set_cookie.contains("Max-Age=0"),
        "cookie cleared: {set_cookie}"
    );
    let resp = app
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
        StatusCode::UNAUTHORIZED,
        "and the epoch bump dropped it server-side (amendment §B)"
    );
}

/// A request with a valid session cookie reaches its handler with the login ID
/// that cookie carries.
#[tokio::test]
async fn the_guard_hands_the_login_id_to_the_request() {
    let session_epoch = epoch::SessionEpoch::in_memory_detached();
    let policy = auth::AuthPolicy::Session(std::sync::Arc::new(auth::SessionAuth {
        token: "tok".to_string(),
        totp: rfc_seed(),
        password: None,
        epoch: session_epoch.clone(),
        revoked: auth::RevokedLogins::default(),
    }));
    let state = auth::AuthState::fixed(policy, session_epoch.clone());
    let probe = |req: Request<Body>| async move {
        req.extensions()
            .get::<cookie::LoginId>()
            .map(ToString::to_string)
            .unwrap_or_default()
    };
    let app = Router::new()
        .route("/api/probe", axum::routing::get(probe))
        .layer(axum::middleware::from_fn_with_state(state, require_auth));
    let login = cookie::LoginId::mint();
    let now = now_unix();
    let value = cookie::sign(
        "tok",
        session_epoch.get(),
        cookie::SessionKind::Standard,
        now,
        now + 600,
        login,
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/probe")
                .header(header::COOKIE, format!("ralphy_session={value}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_text(resp).await, login.to_string());
}

/// After a revoke, the guard refuses the cookie of the revoked login and still
/// lets in the cookie of another login.
#[tokio::test]
async fn a_revoked_login_is_refused_and_another_is_not() {
    let session_epoch = epoch::SessionEpoch::in_memory_detached();
    let policy = auth::AuthPolicy::Session(std::sync::Arc::new(auth::SessionAuth {
        token: "tok".to_string(),
        totp: rfc_seed(),
        password: None,
        epoch: session_epoch.clone(),
        revoked: auth::RevokedLogins::default(),
    }));
    let state = auth::AuthState::fixed(policy, session_epoch.clone());
    let app = Router::new()
        .route("/api/probe", axum::routing::get(|| async { "in" }))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ));
    let now = now_unix();
    let cookie_of = |login| {
        let value = cookie::sign(
            "tok",
            session_epoch.get(),
            cookie::SessionKind::Standard,
            now,
            now + 600,
            login,
        );
        format!("ralphy_session={value}")
    };
    let status = |cookie: String| {
        let app = app.clone();
        async move {
            app.oneshot(
                Request::builder()
                    .uri("/api/probe")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
        }
    };
    let (revoked, other) = (cookie::LoginId::mint(), cookie::LoginId::mint());
    assert_eq!(status(cookie_of(revoked)).await, StatusCode::OK);

    state.revoke_login(revoked, now).expect("revoke the login");

    assert_eq!(status(cookie_of(revoked)).await, StatusCode::UNAUTHORIZED);
    assert_eq!(status(cookie_of(other)).await, StatusCode::OK);
}

/// A successful login answers with the login ID it minted, the same one its
/// cookie carries, so a layer around the route can record it.
#[tokio::test]
async fn a_login_response_carries_its_login_id() {
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
    assert_eq!(resp.status(), StatusCode::OK);
    let login = *resp
        .extensions()
        .get::<cookie::LoginId>()
        .expect("the response carries the login ID");
    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("a Set-Cookie header");
    let value = cookie::from_cookie_header(set_cookie.split(';').next()).expect("a cookie value");
    let claims = cookie::verify_claims("tok", 0, &value, now).expect("the cookie verifies");
    assert_eq!(claims.login, login, "the response and the cookie agree");
}

/// A wrong TOTP code is rejected `401` by `POST /api/login` (the login handler
/// checks the code VALUE, not merely form presence — a presence-only bug would
/// pass the happy-path test above).
#[tokio::test]
async fn session_login_rejects_wrong_code() {
    let resp = session_router("tok")
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("code=000000"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "wrong code → 401");
}

/// The auth layer covers the REMOTE-EXEC WS routes, not just `/api`: an
/// unauthenticated `/ws` request (the page socket, which carries the
/// consoles and the runs) is rejected `401` by the
/// middleware BEFORE reaching the upgrade handler. A `401` here (not the
/// handler's `400`) proves the layer fired, so a future route reordered past
/// the layer would fail this test instead of silently serving an
/// unauthenticated shell/run trigger.
#[tokio::test]
async fn bearer_policy_gates_the_remote_exec_ws_routes() {
    for uri in ["/ws", "/api/usage", "/api/peer/usage"] {
        let resp = router(
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
        )
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "{uri} must be gated by the auth layer, not reach its handler"
        );
    }
}
