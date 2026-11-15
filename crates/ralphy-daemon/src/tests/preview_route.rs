//! `GET /preview/<token>/<path>` (ADR-0089): the token is the only key, and
//! the answer carries its own policy.

use super::*;

/// A repo folder with one page, and a token for it.
fn page_and_token() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("a temp dir is writable");
    std::fs::write(dir.path().join("page.html"), b"<p>page</p>").expect("the page is written");
    std::fs::write(dir.path().join(".env"), b"SECRET=1").expect("the secret is written");
    let token = preview::PREVIEWS.open(dir.path().to_path_buf(), Instant::now());
    (dir, token)
}

fn request(path: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut req = Request::builder()
        .uri(path)
        .header(header::HOST, "127.0.0.1:7257");
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    req.body(Body::empty()).expect("the request builds")
}

/// A session daemon, and a request with no cookie and no bearer: the token
/// alone opens the file, with `Origin: null` too, as Firefox sends it.
#[tokio::test]
async fn a_preview_file_is_served_to_its_token_with_its_own_policy() {
    let (_dir, token) = page_and_token();
    let path = format!("/preview/{token}/page.html");
    for origin in [&[][..], &[("origin", "null")][..]] {
        let resp = session_router("tok")
            .oneshot(request(&path, origin))
            .await
            .expect("the router answers");
        assert_eq!(resp.status(), StatusCode::OK, "{origin:?}");
        let h = resp.headers();
        assert_eq!(h[header::CONTENT_TYPE], "text/html");
        assert_eq!(
            h[header::CONTENT_SECURITY_POLICY],
            preview::policy(
                &format!("http://127.0.0.1:7257/preview/{token}/"),
                preview::Web::default()
            )
            .as_str()
        );
        assert_eq!(h.get(header::X_FRAME_OPTIONS), None);
        assert_eq!(h[header::CACHE_CONTROL], "no-store");
        assert_eq!(h[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(h[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(body_string(resp).await, "<p>page</p>");
    }
    // The control: the same `Origin` on an API route is refused.
    let api = session_router("tok")
        .oneshot(request("/api/session", &[("origin", "null")]))
        .await
        .expect("the router answers");
    assert_eq!(api.status(), StatusCode::FORBIDDEN);
}

/// Behind a front, the policy names the address the browser uses.
#[tokio::test]
async fn the_policy_names_the_host_and_scheme_of_the_front() {
    let (_dir, token) = page_and_token();
    let front = [
        ("x-forwarded-proto", "https"),
        ("x-forwarded-host", "abc-7257.brs.devtunnels.ms"),
    ];
    let resp = session_router("tok")
        .oneshot(request(&format!("/preview/{token}/page.html"), &front))
        .await
        .expect("the router answers");
    assert_eq!(
        resp.headers()[header::CONTENT_SECURITY_POLICY],
        preview::policy(
            &format!("https://abc-7257.brs.devtunnels.ms/preview/{token}/"),
            preview::Web::default()
        )
        .as_str()
    );
}

#[tokio::test]
async fn a_preview_file_is_refused_without_its_token_or_outside_the_list() {
    let (_dir, token) = page_and_token();
    let cookie = login_set_cookie("").await;
    let session = cookie.split(';').next().expect("the cookie has a value");
    // [path, headers]: each is refused, a valid session included.
    let rows: [(String, &[(&str, &str)]); 4] = [
        ("/preview/not-a-token/page.html".to_owned(), &[]),
        (
            "/preview/not-a-token/page.html".to_owned(),
            &[("cookie", session)],
        ),
        (format!("/preview/{token}/.env"), &[]),
        (format!("/preview/{token}/missing.html"), &[]),
    ];
    for (path, headers) in rows {
        let resp = session_router("tok")
            .oneshot(request(&path, headers))
            .await
            .expect("the router answers");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(!body_string(resp).await.contains("SECRET"), "{path}");
    }
    // A host this daemon does not answer on is refused before the token.
    let mut other = request(&format!("/preview/{token}/page.html"), &[]);
    other.headers_mut().insert(
        header::HOST,
        "evil.example".parse().expect("a header value"),
    );
    let resp = session_router("tok")
        .oneshot(other)
        .await
        .expect("the router answers");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// ADR-0089 D7: a frame cannot navigate to a data endpoint. The page's own
/// read of the same endpoint, and a frame that asks for the shell, pass.
#[tokio::test]
async fn a_frame_that_navigates_to_a_data_endpoint_is_refused() {
    // [path, Sec-Fetch-Dest, status]
    let rows = [
        ("/api/session", Some("iframe"), StatusCode::FORBIDDEN),
        ("/api/session", Some("frame"), StatusCode::FORBIDDEN),
        ("/api/session", Some("embed"), StatusCode::FORBIDDEN),
        ("/api/session", Some("object"), StatusCode::FORBIDDEN),
        ("/ws", Some("iframe"), StatusCode::FORBIDDEN),
        ("/api/session", Some("empty"), StatusCode::OK),
        ("/api/session", Some("document"), StatusCode::OK),
        ("/api/session", None, StatusCode::OK),
        ("/", Some("iframe"), StatusCode::OK),
    ];
    for (path, dest, want) in rows {
        let headers: Vec<(&str, &str)> = dest.map(|d| ("sec-fetch-dest", d)).into_iter().collect();
        let resp = session_router("tok")
            .oneshot(request(path, &headers))
            .await
            .expect("the router answers");
        assert_eq!(resp.status(), want, "{path} {dest:?}");
    }
}
