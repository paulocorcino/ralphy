//! The Telegram config of a daemon is the one in its store (ADR-0007 D2,
//! amendment of 2026-11-03): a scratch store has no bot, even on a machine
//! whose operator has one in the old config file, and `GET /api/telegram`
//! says why it is off.
//!
//! The store is process-global environment; nextest runs each test in its
//! own process.

#![expect(
    clippy::disallowed_methods,
    reason = "nextest runs each test in its own process, so a test may change its environment"
)]

#[path = "support/golden.rs"]
mod golden;

use std::path::PathBuf;

use axum::body::Body;
use axum::http::Request;
use ralphy_daemon::telegram_state::{self, legacy, OffReason, TelegramState};
use ralphy_daemon::{auth, router, StorePaths};
use tower::ServiceExt;

/// A store named by `RALPHY_DAEMON_DIR` reads its own `telegram.toml`, and
/// never takes the operator's old file.
#[test]
fn a_scratch_store_has_no_bot() {
    let store = tempfile::tempdir().expect("a temporary store");
    std::env::set_var("RALPHY_DAEMON_DIR", store.path());
    let config = telegram_state::config_path().expect("the config path");
    assert_eq!(config, store.path().join("telegram.toml"));
    let moved = legacy::take_legacy_config().expect("the copy");
    assert_eq!(moved, legacy::Moved::NotOperatorStore);
    assert!(
        !config.exists(),
        "a scratch store never gets the operator's bot"
    );
    assert_eq!(
        telegram_state::state_at(&config),
        TelegramState::off(OffReason::NotSetUp)
    );
}

/// A test that moves the home folder with `USERPROFILE` and `HOME` makes a
/// store there. It never takes the operator's old file: on Windows the old
/// file and the operator's store follow the known folder, not `USERPROFILE`;
/// elsewhere both follow `HOME`, so the old file is not there either.
#[test]
fn a_moved_home_folder_takes_no_bot() {
    let home = tempfile::tempdir().expect("a temporary home");
    std::env::remove_var("RALPHY_DAEMON_DIR");
    std::env::set_var("USERPROFILE", home.path());
    std::env::set_var("HOME", home.path());
    let store = home.path().join(".ralphy");
    std::fs::create_dir(&store).expect("the store");
    assert_eq!(auth::store_dir().expect("the store"), store);
    legacy::take_legacy_config().expect("the copy");
    assert!(
        !telegram_state::config_path_in(&store).exists(),
        "a moved home folder never gets the operator's bot"
    );
}

/// `GET /api/telegram` of a daemon with no bot, written as the shared reply
/// the workbench tests read.
#[tokio::test]
async fn the_reply_of_a_daemon_with_no_bot_says_why() {
    let store = tempfile::tempdir().expect("a temporary store");
    let (_tx, shutdown) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        PathBuf::from("does-not-exist"),
        store.path().join("peers"),
        StorePaths {
            telegram_config: telegram_state::config_path_in(store.path()),
            ..Default::default()
        },
        std::time::Instant::now(),
        shutdown,
        auth::AuthState::localhost(),
    );
    let request = Request::builder()
        .uri("/api/telegram")
        .body(Body::empty())
        .expect("a request");
    let response = app.oneshot(request).await.expect("the router answers");
    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), 1 << 16)
        .await
        .expect("the body");
    let reply: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    assert_eq!(
        reply,
        serde_json::json!({ "state": "off", "why": "not-set-up" })
    );
    golden::check("api-telegram", reply, &[]);
}
