//! A token for a file of a peer: the peer reads the file, and this daemon
//! writes the policy (ADR-0089, amendment 2026-11-23).

use axum::http::Request;
use tower::ServiceExt;

use super::*;
use crate::routes::api_file_bytes::peer_tests::{
    bearer_auth, descriptor, router_on, serve as serve_peer, store_with_demo, TOKEN,
};

/// A peer with the repo `demo` that holds `page.html`, and a local token for it.
async fn peer_page_token() -> (tempfile::TempDir, String) {
    let (dir, registry) = store_with_demo();
    let repo = dir.path().join("repo");
    std::fs::write(repo.join("page.html"), b"<p>peer</p>").expect("the page is written");
    std::fs::write(repo.join(".env"), b"SECRET=1").expect("the secret is written");
    let port = serve_peer(router_on(&registry, bearer_auth())).await;
    (
        dir,
        preview::PREVIEWS.open(peer_scope(port), Instant::now()),
    )
}

fn peer_scope(port: u16) -> preview::Scope {
    preview::Scope::Peer {
        peer: Box::new(descriptor(port)),
        slug: "demo".to_owned(),
        checkout: None,
    }
}

async fn ask(token: &str, rel: &str) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7257"));
    serve(
        auth::AuthState::localhost(),
        token.to_owned(),
        rel.to_owned(),
        headers,
    )
    .await
}

#[tokio::test]
async fn a_peer_file_comes_with_the_policy_of_this_daemon() {
    let (_dir, token) = peer_page_token().await;
    let resp = ask(&token, "page.html").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let h = resp.headers();
    assert_eq!(h[header::CONTENT_TYPE], "text/html");
    let local = format!("http://127.0.0.1:7257/preview/{token}/");
    assert_eq!(
        h[header::CONTENT_SECURITY_POLICY],
        preview::policy(&local, preview::Web::default()).as_str()
    );
    assert_eq!(h[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("the body");
    assert_eq!(body.as_ref(), b"<p>peer</p>");
}

#[tokio::test]
async fn a_file_the_peer_refuses_or_a_gone_peer_is_not_found() {
    let (_dir, token) = peer_page_token().await;
    let secret = ask(&token, ".env").await;
    assert_eq!(secret.status(), StatusCode::NOT_FOUND);
    assert!(secret
        .headers()
        .contains_key(header::CONTENT_SECURITY_POLICY));

    // A port nobody listens on.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("the bound address").port();
    drop(listener);
    let token = preview::PREVIEWS.open(peer_scope(port), Instant::now());
    assert_eq!(
        ask(&token, "page.html").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_peer_preview_route_needs_the_peer_key() {
    let (dir, registry) = store_with_demo();
    std::fs::write(dir.path().join("repo").join("page.html"), b"<p>peer</p>")
        .expect("the page is written");
    let ask = |key: Option<&str>| {
        let mut req = Request::builder().uri("/api/peer/preview?repo=demo&path=page.html");
        if let Some(key) = key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        router_on(&registry, bearer_auth()).oneshot(req.body(Body::empty()).expect("a request"))
    };
    let refused = ask(None).await.expect("the router answers");
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    let served = ask(Some(TOKEN)).await.expect("the router answers");
    assert_eq!(served.status(), StatusCode::OK);
    assert_eq!(served.headers()[header::CONTENT_TYPE], "text/html");
}
