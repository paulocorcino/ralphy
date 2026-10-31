//! The root page, the session avatar, the identity route and the peer handshake.

use super::*;
use crate::serve::{announce_peer, announced_descriptor};
use std::net::SocketAddr;

#[tokio::test]
async fn root_serves_the_embedded_page() {
    let resp = get("/").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    // Anchored on the document TITLE, not on chrome text. The old anchor was
    // the login card's "ralphy daemon" brand, which made a copy edit on one
    // screen look like a broken asset pipeline — and "daemon" left that
    // brand naming the process rather than the thing being logged into.
    assert!(
        body.contains("<title>Ralphy</title>"),
        "the page must identify the daemon; got: {body}"
    );
    // An installed app's window title is "<manifest name> - <page title>"
    // unless the page title starts with the name (Chromium
    // `WebAppBrowserController::GetTitle`), so a name that differs from the
    // title shows the product twice.
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../assets/ui/manifest.webmanifest"))
            .expect("the manifest is valid JSON, pinned by the install test");
    assert_eq!(
        manifest["name"], "Ralphy",
        "the installed app name must equal the page title"
    );
    assert!(
        body.contains(r#"x-data="shell""#),
        "the workbench shell HTML must render at the root"
    );
    let resp = get("/app.js").await;
    assert_eq!(resp.status(), StatusCode::OK, "GET /app.js → 200");
}

/// `/api/session` is allowlisted pre-login, so what it carries is what an
/// UNAUTHENTICATED caller can read. It must carry the avatar (the login card
/// wears this daemon's face) and it must NOT carry the name — the gate stays
/// opaque about everything the operator wrote themselves.
#[tokio::test]
async fn api_session_carries_the_avatar_but_never_the_name() {
    let id = identity::Identity {
        id: ulid::Ulid::nil(),
        name: "anvil".into(),
        avatar: "🐙".into(),
    };
    let resp = router(
        Some(id),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/session")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("🐙"), "must carry the avatar; got: {body}");
    assert!(
        !body.contains("anvil"),
        "must NOT carry the name; got: {body}"
    );
}

/// An un-baptized daemon has no avatar, and `skip_serializing_if` keeps the
/// key off the wire entirely — the SPA's `if (s.avatar)` then leaves the
/// fallback mark in place rather than painting an empty box.
#[tokio::test]
async fn api_session_omits_the_avatar_when_unbaptized() {
    let body = body_string(get_local("/api/session").await).await;
    assert!(
        !body.contains("avatar"),
        "no avatar key expected; got: {body}"
    );
}

#[tokio::test]
async fn unknown_path_is_404() {
    let resp = get("/no-such-asset").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// The page socket `/ws` carries the consoles, the runs and the file tree:
/// the three sockets that carried them before are no longer served.
#[tokio::test]
async fn the_old_socket_routes_are_gone() {
    for path in ["/ws/command", "/ws/tree", "/ws/session"] {
        let request = Request::builder()
            .uri(path)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Body::empty())
            .expect("a static request builds");
        let resp = router(
            None,
            PathBuf::from("does-not-exist"),
            PathBuf::from("does-not-exist"),
            StorePaths::default(),
            Instant::now(),
            idle_shutdown(),
            auth::AuthState::localhost(),
        )
        .oneshot(request)
        .await
        .expect("the router answers");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "GET {path}");
    }
}

#[tokio::test]
async fn api_identity_route_returns_name_and_avatar() {
    let id = identity::Identity {
        id: ulid::Ulid::nil(),
        name: "anvil".into(),
        avatar: "🐙".into(),
    };
    let resp = router(
        Some(id),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/identity")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("anvil"),
        "body must carry the name; got: {body}"
    );
    assert!(
        body.contains("🐙"),
        "body must carry the avatar; got: {body}"
    );

    let resp = router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/identity")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[test]
fn announced_descriptor_advertises_a_nudge_only_inside_wsl() {
    let id = identity::Identity {
        id: ulid::Ulid::nil(),
        name: "anvil".into(),
        avatar: "🐙".into(),
    };
    let inside = announced_descriptor(&id, 7443, Some("Ubuntu-22.04"), "tok".into());
    assert_eq!(inside.environment, "WSL: Ubuntu-22.04");
    assert_eq!(
        inside.nudge,
        Some(peer::NudgeSpec {
            distro: "Ubuntu-22.04".into(),
            unit: autostart::UNIT_NAME.into(),
        }),
        "a WSL daemon advertises how to wake it"
    );
    assert_eq!(
        inside.address, "127.0.0.1",
        "the peer transport is loopback"
    );
    assert_eq!(inside.port, 7443, "the BOUND port is announced");
    assert_eq!(inside.token, "tok", "the resolved token is announced as-is");
    assert_eq!(inside.daemon_id, id.id.to_string());

    let outside = announced_descriptor(&id, 7443, None, "tok".into());
    assert_eq!(
        outside.nudge, None,
        "no `wsl.exe` can reach a non-WSL daemon, so it advertises no nudge"
    );
    assert_ne!(outside.environment, "WSL: Ubuntu-22.04");
    assert_eq!(
        outside.os,
        std::env::consts::OS,
        "the OS family is announced for the workbench icon"
    );
}

/// Announcing must never touch the auth policy: it takes the token it is
/// given rather than minting over it (AC5).
#[test]
fn announce_skips_an_un_baptized_daemon_and_a_non_loopback_bind() {
    let dir = tempfile::tempdir().unwrap();
    let loopback = SocketAddr::from(([127, 0, 0, 1], 7443));
    announce_peer(
        &[dir.path().to_path_buf()],
        None,
        loopback,
        Some("tok".into()),
    );
    assert!(
        !dir.path().join("peers").exists(),
        "an un-baptized daemon announces nothing"
    );

    let id = identity::Identity {
        id: ulid::Ulid::nil(),
        name: "anvil".into(),
        avatar: "🐙".into(),
    };
    announce_peer(
        &[dir.path().to_path_buf()],
        Some(&id),
        SocketAddr::from(([10, 0, 0, 5], 7443)),
        Some("tok".into()),
    );
    assert!(
        !dir.path().join("peers").exists(),
        "a daemon that does not listen on loopback cannot be a peer"
    );

    // The happy path, and a SECOND store whose parent is a file — the write
    // fails there and must not stop the good one.
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, "not a directory").unwrap();
    announce_peer(
        &[blocked, dir.path().to_path_buf()],
        Some(&id),
        loopback,
        Some("tok-given".into()),
    );
    let written = dir
        .path()
        .join("peers")
        .join(format!("{}.toml", ulid::Ulid::nil()));
    let back: peer::PeerDescriptor =
        toml::from_str(&std::fs::read_to_string(&written).unwrap()).unwrap();
    assert_eq!(
        back.token, "tok-given",
        "the resolved token is announced, never re-minted over"
    );
}

#[tokio::test]
async fn api_peer_hello_serves_the_handshake_and_404s_un_baptized() {
    let id = identity::Identity {
        id: ulid::Ulid::nil(),
        name: "anvil".into(),
        avatar: "🐙".into(),
    };
    let resp = router(
        Some(id),
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/peer/hello")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("\"daemon_id\":\"00000000000000000000000000\""),
        "the handshake must carry the daemon_id; got: {body}"
    );
    assert!(
        body.contains(&format!(
            "\"protocol_version\":{}",
            peer::PEER_PROTOCOL_VERSION
        )),
        "the handshake must carry the peer protocol version; got: {body}"
    );
    assert!(
        body.contains("\"environment\":\"") && !body.contains("\"environment\":\"\""),
        "the handshake must name a non-empty environment; got: {body}"
    );

    // Un-baptized: 404, matching `/api/identity` — nothing for a peer to key on.
    let resp = router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/peer/hello")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
