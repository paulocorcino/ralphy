use std::path::PathBuf;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

use crate::{auth, router, StorePaths};

#[path = "../../../tests/support/golden.rs"]
mod golden;

async fn state(config: &std::path::Path) -> serde_json::Value {
    let (_tx, shutdown) = tokio::sync::watch::channel(false);
    let response = router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths {
            telegram_config: config.to_path_buf(),
            ..Default::default()
        },
        std::time::Instant::now(),
        shutdown,
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/telegram")
            .body(Body::empty())
            .expect("a request"),
    )
    .await
    .expect("the router answers");
    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), 1 << 16)
        .await
        .expect("the body");
    serde_json::from_slice(&body).expect("JSON")
}

#[tokio::test]
async fn the_telegram_state_follows_the_config() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let config = dir.path().join("telegram.toml");
    assert_eq!(state(&config).await, serde_json::json!({ "state": "off" }));

    std::fs::write(&config, "token = \"123:abc\"\n").expect("write the config");
    assert_eq!(
        state(&config).await,
        serde_json::json!({ "state": "off" }),
        "a bot with no chat yet sends nothing"
    );

    std::fs::write(&config, "token = \"123:abc\"\nchat_id = 42\n").expect("write the config");
    let sends = state(&config).await;
    assert_eq!(sends, serde_json::json!({ "state": "sends" }));
    golden::check("api-telegram", sends, &[]);
}

/// The reply of `request` on a router over the config at `config`.
async fn reply_of(
    app: axum::Router,
    request: Request<Body>,
) -> (axum::http::StatusCode, serde_json::Value) {
    let response = app.oneshot(request).await.expect("the router answers");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 16)
        .await
        .expect("the body");
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

#[tokio::test]
async fn a_claimed_peer_sends_through_its_main() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let config = dir.path().join("telegram.toml");
    std::fs::write(&config, "token = \"123:abc\"\nchat_id = 42\n").expect("write the config");
    let (_tx, shutdown) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        dir.path().join("repos.toml"),
        PathBuf::from("does-not-exist"),
        StorePaths {
            telegram_config: config,
            ..Default::default()
        },
        std::time::Instant::now(),
        shutdown,
        auth::AuthState::localhost(),
    );
    let get = |uri: &str| {
        Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("a request")
    };
    let (_, before) = reply_of(app.clone(), get("/api/telegram")).await;
    assert_eq!(
        before,
        serde_json::json!({ "state": "sends" }),
        "its own bot"
    );

    let claim = Request::builder()
        .method("POST")
        .uri("/api/telegram/main")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"daemon_id":"m"}"#))
        .expect("a request");
    let (status, claimed) = reply_of(app.clone(), claim).await;
    assert_eq!(status, 200);
    golden::check("api-telegram-main", claimed, &[]);

    let through = serde_json::json!({ "state": "through-main", "main": "m" });
    let (_, joined) = reply_of(app.clone(), get("/api/telegram")).await;
    assert_eq!(joined, through, "the claim wins over its own bot");
    let (_, local) = reply_of(app, get("/api/telegram?local=1")).await;
    assert_eq!(local, through);
}

/// The status of a claim of `main` on a daemon whose id is `own`, with a bot
/// when `bot` is set, and the peers folder holding `peers`.
async fn claim_status(own: &str, bot: bool, peers: &[&str], main: &str) -> u16 {
    let dir = tempfile::tempdir().expect("a temp dir");
    let config = dir.path().join("telegram.toml");
    if bot {
        std::fs::write(
            &config,
            "token = \"123:abc\"
chat_id = 42
",
        )
        .expect("write the config");
    }
    for id in peers {
        let descriptor = crate::peer::PeerDescriptor {
            daemon_id: (*id).to_string(),
            name: "peer".to_string(),
            avatar: String::new(),
            address: "127.0.0.1".to_string(),
            port: 1,
            environment: "WSL: Ubuntu".to_string(),
            os: String::new(),
            token: "peer-token".to_string(),
            protocol_version: crate::peer::PEER_PROTOCOL_VERSION,
            tunnel: None,
            nudge: None,
        };
        crate::peer::write_descriptor(dir.path(), &descriptor).expect("write the peer");
    }
    let shared = super::TelegramShared {
        config,
        claim: Default::default(),
        peers_dir: dir.path().join("peers"),
        daemon_id: Some(own.to_string()),
        name: None,
    };
    let body = super::ClaimBody {
        daemon_id: main.to_string(),
    };
    super::claim_route(shared, body).await.status().as_u16()
}

#[tokio::test]
async fn a_claim_names_the_id_of_another_daemon() {
    let too_long = "a".repeat(super::MAX_MAIN_ID_BYTES + 1);
    let longest = "a".repeat(super::MAX_MAIN_ID_BYTES);
    // (case, the id claimed, the status)
    let rows: [(&str, &str, u16); 7] = [
        ("empty", "", 400),
        ("over the longest id", &too_long, 400),
        ("a space", "01 B", 400),
        ("a slash", "01/B", 400),
        ("this daemon's own id", "01OWN", 400),
        ("the longest id", &longest, 200),
        ("letters, digits, dash and underscore", "01b-_9", 200),
    ];
    for (case, main, status) in rows {
        assert_eq!(
            claim_status("01OWN", false, &[], main).await,
            status,
            "{case}"
        );
    }
}

/// (case, own id, has a bot, the peers folder, the claimer, the status)
type MutualRow<'a> = (&'a str, &'a str, bool, &'a [&'a str], &'a str, u16);

#[tokio::test]
async fn two_daemons_that_are_each_others_peers_do_not_both_refuse() {
    let rows: [MutualRow; 5] = [
        (
            "own id first, claimer is a peer",
            "01B",
            true,
            &["01C"],
            "01C",
            409,
        ),
        (
            "claimer first, claimer is a peer",
            "01C",
            true,
            &["01B"],
            "01B",
            200,
        ),
        ("own id first, no bot", "01B", false, &["01C"], "01C", 200),
        (
            "own id first, claimer is no peer",
            "01B",
            true,
            &["01D"],
            "01C",
            200,
        ),
        ("own id first, no peers", "01B", true, &[], "01C", 200),
    ];
    for (case, own, bot, peers, main, status) in rows {
        assert_eq!(claim_status(own, bot, peers, main).await, status, "{case}");
    }
}

#[tokio::test]
async fn a_claim_body_over_the_cap_is_refused() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (_tx, shutdown) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        dir.path().join("repos.toml"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        std::time::Instant::now(),
        shutdown,
        auth::AuthState::localhost(),
    );
    let claim = |id: String| {
        Request::builder()
            .method("POST")
            .uri("/api/telegram/main")
            .header("content-type", "application/json")
            .body(Body::from(format!(r#"{{"daemon_id":"{id}"}}"#)))
            .expect("a request")
    };
    let (status, _) = reply_of(app.clone(), claim("a".repeat(super::CLAIM_MAX_BYTES))).await;
    assert_eq!(status, 413);
    let (status, _) = reply_of(app, claim("m".to_string())).await;
    assert_eq!(status, 200, "a claim of the size the page sends is read");
}
