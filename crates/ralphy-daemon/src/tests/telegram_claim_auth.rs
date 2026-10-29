//! `POST /api/telegram/main` is a machine route: a page's session cookie
//! does not claim a daemon, a daemon's bearer does.

use super::*;

fn claim(auth: &[(header::HeaderName, &str)]) -> Request<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/telegram/main")
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in auth {
        req = req.header(name, *value);
    }
    req.body(Body::from(r#"{"daemon_id":"m"}"#)).unwrap()
}

#[tokio::test]
async fn a_claim_needs_the_bearer_of_a_daemon() {
    let app = session_router("tok");
    let now = now_unix();
    let code = rfc_seed().code_at(now / 30);
    let login = app
        .clone()
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
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .expect("a session cookie")
        .to_string();

    let page = app
        .clone()
        .oneshot(claim(&[(header::COOKIE, &cookie)]))
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::FORBIDDEN, "a page cannot claim");
    // The cookie gets the request past the guard; the claim still asks for
    // the daemon's own bearer, not any bearer.
    let wrong = app
        .clone()
        .oneshot(claim(&[
            (header::COOKIE, &cookie),
            (header::AUTHORIZATION, "Bearer junk"),
        ]))
        .await
        .unwrap();
    assert_eq!(
        wrong.status(),
        StatusCode::FORBIDDEN,
        "a cookie and a wrong bearer cannot claim"
    );
    let peer = app
        .oneshot(claim(&[(header::AUTHORIZATION, "Bearer tok")]))
        .await
        .unwrap();
    assert_eq!(peer.status(), StatusCode::OK, "a daemon's bearer claims");

    let store = tempfile::tempdir().unwrap();
    let localhost = desk_router(store.path());
    let bare = localhost.clone().oneshot(claim(&[])).await.unwrap();
    assert_eq!(
        bare.status(),
        StatusCode::FORBIDDEN,
        "under the localhost policy a claim still carries a bearer"
    );
    let peer = localhost
        .oneshot(claim(&[(header::AUTHORIZATION, "Bearer any")]))
        .await
        .unwrap();
    assert_eq!(peer.status(), StatusCode::OK);
}
