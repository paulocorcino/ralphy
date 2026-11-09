use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ralphy_telegram::client::Transport;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{bot_routes, BotShared};
use crate::attention::telegram::AnyTransport;

const GOOD: &str = "123:good";

/// The Bot API as the routes see it: `GOOD` is the token of `@ralphy_bot`,
/// `getUpdates` answers `updates`, and each `sendMessage` is kept.
#[derive(Clone, Default)]
struct Api {
    updates: Arc<Mutex<Value>>,
    sent: Arc<Mutex<Vec<Value>>>,
}

struct Call {
    api: Api,
    token: String,
}

impl Transport for Call {
    fn get(&self, method: &str) -> Result<Value> {
        if self.token != GOOD {
            return Ok(json!({ "ok": false, "description": "Unauthorized" }));
        }
        match method {
            "getMe" => Ok(json!({ "ok": true, "result": { "username": "ralphy_bot" } })),
            "getUpdates" => Ok(self.api.updates.lock().expect("lock").clone()),
            _ => bail!("no such method {method}"),
        }
    }

    fn post(&self, method: &str, body: Value) -> Result<Value> {
        assert_eq!(method, "sendMessage");
        self.api.sent.lock().expect("lock").push(body);
        Ok(json!({ "ok": true, "result": { "message_id": 1 } }))
    }
}

fn app(config: &Path, api: &Api) -> axum::Router {
    let api = api.clone();
    bot_routes(BotShared {
        config: config.to_path_buf(),
        claim: Default::default(),
        transport: Arc::new(move |token| {
            AnyTransport(Box::new(Call {
                api: api.clone(),
                token,
            }))
        }),
    })
}

fn temp_config() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let config = dir.path().join("telegram.toml");
    (dir, config)
}

/// `method uri` with `body`, from a browser on this computer unless `host`
/// says otherwise.
async fn call(
    app: axum::Router,
    (method, uri): (&str, &str),
    body: Option<Value>,
    host: &str,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let response = app
        .oneshot(request.body(body).expect("a request"))
        .await
        .expect("the router answers");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 16)
        .await
        .expect("the body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).expect("JSON")
}

#[tokio::test]
async fn the_page_sets_up_the_bot_in_the_steps_of_the_cli() {
    let (_dir, config) = temp_config();
    let api = Api::default();
    let local = "127.0.0.1:7878";

    let (status, body) = call(
        app(&config, &api),
        ("GET", "/api/telegram/bot"),
        None,
        local,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json_of(&body),
        json!({ "state": "off", "why": "not-set-up", "bot": null, "chat": null, "answered": true })
    );

    let bad = Some(json!({ "token": "999:bad" }));
    let (status, _) = call(
        app(&config, &api),
        ("POST", "/api/telegram/bot"),
        bad,
        local,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a token Telegram refuses is not kept"
    );
    assert!(!config.exists());

    let good = Some(json!({ "token": format!(" {GOOD} ") }));
    let (status, body) = call(
        app(&config, &api),
        ("POST", "/api/telegram/bot"),
        good,
        local,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains(GOOD), "no reply holds the token: {body}");
    assert_eq!(json_of(&body)["bot"], "ralphy_bot");
    assert_eq!(json_of(&body)["why"], "no-chat");
    assert!(crate::owner_only::is_owner_only(&config).expect("the config's mode"));

    let (status, _) = call(
        app(&config, &api),
        ("POST", "/api/telegram/chat"),
        None,
        local,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "no /start yet");

    *api.updates.lock().expect("lock") = json!({ "ok": true, "result": [
        { "update_id": 1, "message": { "text": "/start", "chat": { "id": 4242 } } }
    ]});
    let (status, body) = call(
        app(&config, &api),
        ("POST", "/api/telegram/chat"),
        None,
        local,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_of(&body)["chat"], 4242);
    assert_eq!(json_of(&body)["state"], "sends");

    let (status, _) = call(
        app(&config, &api),
        ("POST", "/api/telegram/test"),
        None,
        local,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(api.sent.lock().expect("lock")[0]["chat_id"], 4242);

    let same = Some(json!({ "token": GOOD }));
    let (_, body) = call(
        app(&config, &api),
        ("POST", "/api/telegram/bot"),
        same,
        local,
    )
    .await;
    assert_eq!(
        json_of(&body)["chat"],
        4242,
        "the same token keeps its chat"
    );

    let (status, body) = call(
        app(&config, &api),
        ("DELETE", "/api/telegram/bot"),
        None,
        local,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_of(&body)["why"], "not-set-up");
    assert!(!config.exists());
}

#[tokio::test]
async fn a_token_that_may_cross_a_network_in_clear_text_is_refused() {
    let (_dir, config) = temp_config();
    let api = Api::default();
    let good = Some(json!({ "token": GOOD }));
    let (status, _) = call(
        app(&config, &api),
        ("POST", "/api/telegram/bot"),
        good,
        "192.168.0.10:7878",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!config.exists());
}

#[tokio::test]
async fn a_test_message_needs_a_chat() {
    let (_dir, config) = temp_config();
    std::fs::write(&config, format!("token = \"{GOOD}\"\n")).expect("write the config");
    let api = Api::default();
    let (status, _) = call(
        app(&config, &api),
        ("POST", "/api/telegram/test"),
        None,
        "localhost",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(api.sent.lock().expect("lock").is_empty());
}
