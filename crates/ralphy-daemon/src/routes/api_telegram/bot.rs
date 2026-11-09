//! The setup of this daemon's bot from Settings (docs/adr/0085, amendment of
//! 2026-11-07), in the steps of `ralphy telegram setup`:
//! `GET`/`POST`/`DELETE /api/telegram/bot`, `POST /api/telegram/chat` and
//! `POST /api/telegram/test`. No reply holds the token (ADR-0072 D7,
//! amendment of 2026-11-07): the page sees the bot's name, the chat and the
//! state.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use ralphy_telegram::client::{detect_chat_id, BotClient};
use ralphy_telegram::config::TelegramConfig;

use crate::attention::telegram::{AnyTransport, Transports};
use crate::routes::{blocking_read, request_may_carry_a_secret};
use crate::telegram_state::{self, store, SharedClaim, TelegramState};

/// The largest body `POST /api/telegram/bot` reads (ADR-0072 D11). A bot
/// token is about 46 characters.
const TOKEN_BODY_MAX_BYTES: usize = 1024;

/// The longest token the route takes.
const MAX_TOKEN_BYTES: usize = 128;

/// The text of `POST /api/telegram/test`.
const TEST_TEXT: &str = "Ralphy test message ✓";

/// What the setup routes read: the config, the claim on this daemon, and
/// what builds the transport for a token.
#[derive(Clone)]
pub(crate) struct BotShared {
    pub(crate) config: PathBuf,
    pub(crate) claim: SharedClaim,
    pub(crate) transport: Transports,
}

/// The body of `POST /api/telegram/bot`.
#[derive(serde::Deserialize)]
pub(crate) struct TokenBody {
    pub(crate) token: String,
}

/// What the page shows. `bot` is the bot's name, `None` when there is no
/// token or Telegram did not answer for it (`answered: false`).
#[derive(serde::Serialize)]
pub(crate) struct BotReply {
    #[serde(flatten)]
    pub(crate) state: TelegramState,
    pub(crate) bot: Option<String>,
    pub(crate) chat: Option<i64>,
    pub(crate) answered: bool,
}

/// A refusal the page shows as it is: `{"error": text}`.
pub(crate) struct Refusal(StatusCode, &'static str);

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

/// A refusal for a failure the operator cannot see the cause of: the cause
/// is logged. A Bot API error is redacted of the token by its transport.
fn failed(e: &anyhow::Error, status: StatusCode, text: &'static str) -> Refusal {
    tracing::warn!(error = format!("{e:#}"), "{text}");
    Refusal(status, text)
}

fn client(b: &BotShared, token: &str) -> BotClient<AnyTransport> {
    BotClient::new((b.transport)(token.to_string()))
}

/// The bot's name, from `getMe`.
fn name_of(b: &BotShared, token: &str) -> anyhow::Result<String> {
    let me = client(b, token).get_me()?;
    me.get("username")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("getMe named no bot"))
}

/// The config at `path`. A file that cannot be read is logged and is no
/// config here; the state says why.
fn config_at(path: &Path) -> Option<TelegramConfig> {
    match TelegramConfig::load_from(path) {
        Ok(config) => config,
        Err(e) => {
            // Only the outer message: the parser's quotes the line it failed
            // on, which may hold the token.
            tracing::warn!(error = %e, "the Telegram config cannot be read");
            None
        }
    }
}

/// The reply for the config as it is now, asking Telegram for the bot's
/// name when there is a token.
fn reply_now(b: &BotShared) -> BotReply {
    let state = telegram_state::state_with(&b.config, &b.claim, SystemTime::now());
    let config = config_at(&b.config);
    let bot = config.as_ref().and_then(|c| match name_of(b, &c.token) {
        Ok(name) => Some(name),
        Err(e) => {
            tracing::warn!(
                error = format!("{e:#}"),
                "Telegram did not answer for the bot"
            );
            None
        }
    });
    BotReply {
        answered: config.is_none() || bot.is_some(),
        state,
        bot,
        chat: config.and_then(|c| c.chat_id),
    }
}

async fn read(b: BotShared) -> Response {
    answer(blocking_read(move || Ok(reply_now(&b))).await)
}

/// Whether `token` may be a bot token: short, one word, no control bytes.
fn token_is_valid(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_TOKEN_BYTES
        && token.bytes().all(|c| c.is_ascii_graphic())
}

const SAVE_FAILED: &str = "the Telegram settings could not be saved";

/// Keep `token` once `getMe` accepts it. The same token keeps its chat; a
/// new one has none until "Find my chat".
fn save_token(b: &BotShared, token: String) -> Result<BotReply, Refusal> {
    name_of(b, &token).map_err(|e| {
        failed(
            &e,
            StatusCode::BAD_REQUEST,
            "Telegram did not accept this token",
        )
    })?;
    let old = config_at(&b.config);
    let chat_id = old.filter(|c| c.token == token).and_then(|c| c.chat_id);
    store::save_to(&TelegramConfig { token, chat_id }, &b.config)
        .map_err(|e| failed(&e, StatusCode::INTERNAL_SERVER_ERROR, SAVE_FAILED))?;
    Ok(reply_now(b))
}

async fn set_token(b: BotShared, headers: HeaderMap, body: TokenBody) -> Response {
    if !request_may_carry_a_secret(&headers) {
        let text = "a bot token is taken only over https or from this computer";
        return Refusal(StatusCode::FORBIDDEN, text).into_response();
    }
    let token = body.token.trim().to_string();
    if !token_is_valid(&token) {
        return Refusal(StatusCode::BAD_REQUEST, "this is not a bot token").into_response();
    }
    answer(blocking_read(move || save_token(&b, token)).await)
}

/// Keep the chat of the last `/start` the bot received.
fn find_chat(b: &BotShared) -> Result<BotReply, Refusal> {
    let Some(mut config) = config_at(&b.config) else {
        return Err(Refusal(StatusCode::CONFLICT, "no bot is set up"));
    };
    let updates = client(b, &config.token)
        .get_updates()
        .map_err(|e| failed(&e, StatusCode::BAD_GATEWAY, "Telegram did not answer"))?;
    let Some(chat) = detect_chat_id(&updates) else {
        let text = "the bot has no /start yet: send /start to the bot, then try again";
        return Err(Refusal(StatusCode::CONFLICT, text));
    };
    config.chat_id = Some(chat);
    store::save_to(&config, &b.config)
        .map_err(|e| failed(&e, StatusCode::INTERNAL_SERVER_ERROR, SAVE_FAILED))?;
    Ok(reply_now(b))
}

/// Send the test message with the bot this daemon sends with.
fn send_test(b: &BotShared) -> Result<BotReply, Refusal> {
    let no_chat = Refusal(StatusCode::CONFLICT, "the bot has no chat yet");
    let bot = match telegram_state::bot_at(&b.config) {
        Ok(Some(bot)) => bot,
        Ok(None) => return Err(no_chat),
        Err(e) => {
            tracing::warn!(error = %e, "the Telegram config cannot be read");
            return Err(no_chat);
        }
    };
    let text = "Telegram did not take the message";
    client(b, &bot.token)
        .send_message(bot.chat_id, TEST_TEXT)
        .map_err(|e| failed(&e, StatusCode::BAD_GATEWAY, text))?;
    Ok(reply_now(b))
}

/// Remove the bot, as `ralphy telegram disable` does.
fn remove(b: &BotShared) -> Result<BotReply, Refusal> {
    let store_dir = b.config.parent().unwrap_or(Path::new(""));
    let text = "the bot could not be removed";
    store::remove_from(store_dir)
        .map_err(|e| failed(&e, StatusCode::INTERNAL_SERVER_ERROR, text))?;
    Ok(reply_now(b))
}

fn answer(done: Option<Result<BotReply, Refusal>>) -> Response {
    match done {
        Some(Ok(reply)) => Json(reply).into_response(),
        Some(Err(refusal)) => refusal.into_response(),
        None => Refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the request did not finish",
        )
        .into_response(),
    }
}

/// The setup routes over `b`.
pub(crate) fn bot_routes(b: BotShared) -> Router {
    let (r, s, d, c, t) = (b.clone(), b.clone(), b.clone(), b.clone(), b);
    let token =
        move |headers: HeaderMap, Json(body): Json<TokenBody>| set_token(s.clone(), headers, body);
    let bot = get(move || read(r.clone()))
        .post(token)
        .delete(move || {
            let d = d.clone();
            async move { answer(blocking_read(move || remove(&d)).await) }
        })
        .layer(DefaultBodyLimit::max(TOKEN_BODY_MAX_BYTES));
    let chat = move || {
        let c = c.clone();
        async move { answer(blocking_read(move || find_chat(&c)).await) }
    };
    let test = move || {
        let t = t.clone();
        async move { answer(blocking_read(move || send_test(&t)).await) }
    };
    Router::new()
        .route("/api/telegram/bot", bot)
        .route("/api/telegram/chat", post(chat))
        .route("/api/telegram/test", post(test))
}

#[cfg(test)]
mod tests;
