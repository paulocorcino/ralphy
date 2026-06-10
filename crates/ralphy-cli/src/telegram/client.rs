//! A blocking Telegram Bot API client (ADR-0007 D4).
//!
//! The HTTP layer sits behind a [`Transport`] trait so the [`BotClient`] is
//! testable without a network: [`UreqTransport`] is the real blocking `ureq`
//! impl, and tests inject a fake. No async runtime is involved.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// The HTTP transport the [`BotClient`] talks through. Each call names a Bot API
/// method (`getMe`, `sendMessage`, …) and returns the parsed JSON response.
///
/// Abstracting the transport is the only practical way to satisfy "HTTP layer
/// mocked": `ureq` ships no mocking and the workspace carries no mock-HTTP
/// crate, so the client is generic over an injectable transport.
pub trait Transport {
    /// Issue a GET for `method` and return the parsed JSON response.
    fn get(&self, method: &str) -> Result<Value>;
    /// POST `body` as JSON to `method` and return the parsed JSON response.
    fn post(&self, method: &str, body: Value) -> Result<Value>;
}

/// The real blocking transport: every call hits
/// `https://api.telegram.org/bot{token}/{method}` with `ureq`.
pub struct UreqTransport {
    token: String,
}

impl UreqTransport {
    /// Build a transport for `token`.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    fn url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{}", self.token, method)
    }
}

impl Transport for UreqTransport {
    fn get(&self, method: &str) -> Result<Value> {
        let resp = ureq::get(&self.url(method)).call();
        envelope(resp, method)
    }

    fn post(&self, method: &str, body: Value) -> Result<Value> {
        let resp = ureq::post(&self.url(method)).send_json(body);
        envelope(resp, method)
    }
}

/// Turn a `ureq` result into the Bot API's JSON envelope.
///
/// The Bot API reports failures (bad token, bad request) with a non-2xx status
/// AND an `{ "ok": false, "description": ... }` body. `ureq` treats a non-2xx
/// status as `Err(Error::Status(_, resp))` by default, so we must read the body
/// off that error too — otherwise the human-readable `description` is lost and
/// callers only see an opaque HTTP error. Both the success and error-status
/// bodies are parsed and returned for [`result_of`] to interpret.
fn envelope(resp: Result<ureq::Response, ureq::Error>, method: &str) -> Result<Value> {
    match resp {
        Ok(r) => r
            .into_json()
            .with_context(|| format!("parsing {method} response")),
        Err(ureq::Error::Status(_, r)) => r
            .into_json()
            .with_context(|| format!("parsing {method} error response")),
        Err(e) => Err(e).with_context(|| format!("{method} request failed")),
    }
}

/// A thin Bot API client generic over its [`Transport`].
pub struct BotClient<T: Transport> {
    transport: T,
}

impl<T: Transport> BotClient<T> {
    /// Wrap a transport.
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// `getMe` — confirm the token identifies a live bot. Returns the parsed
    /// `result` object, erroring if the API reports `ok: false`.
    pub fn get_me(&self) -> Result<Value> {
        let resp = self.transport.get("getMe")?;
        result_of(resp, "getMe")
    }

    /// `getUpdates` — the raw response (its `result` is the updates array),
    /// used by [`detect_chat_id`] to find the chat that sent `/start`.
    pub fn get_updates(&self) -> Result<Value> {
        self.transport.get("getUpdates")
    }

    /// `sendMessage` — post `text` to `chat_id`. Returns the sent message object.
    pub fn send_message(&self, chat_id: i64, text: &str) -> Result<Value> {
        let resp = self
            .transport
            .post("sendMessage", json!({ "chat_id": chat_id, "text": text }))?;
        result_of(resp, "sendMessage")
    }

    /// `editMessageText` — replace the text of an existing message. Added now so
    /// the next slice's live card (ADR-0007 D3) reuses it; unused this slice.
    #[allow(dead_code)]
    pub fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> Result<Value> {
        let resp = self.transport.post(
            "editMessageText",
            json!({ "chat_id": chat_id, "message_id": message_id, "text": text }),
        )?;
        result_of(resp, "editMessageText")
    }
}

/// Unwrap a Bot API envelope (`{ "ok": bool, "result"|"description": ... }`),
/// returning the `result` on success and an error carrying `description`
/// otherwise.
fn result_of(resp: Value, method: &str) -> Result<Value> {
    if resp.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(resp.get("result").cloned().unwrap_or(Value::Null));
    }
    let desc = resp
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    bail!("{method} failed: {desc}")
}

/// Find the `chat.id` of the most recent inbound `/start` in a `getUpdates`
/// response.
///
/// `setup` prompts the operator to send `/start` to the bot, then polls
/// `getUpdates`; this scans the returned updates for messages whose text begins
/// with `/start` and returns the chat of the LAST such message, so a fresh
/// `/start` from a new chat wins over a stale one in the same backlog. Updates
/// that carry no plain `message` (edited messages, channel posts) are skipped
/// rather than aborting the scan. Restricting to `/start` avoids auto-capturing
/// any chat that merely messages the bot (ADR-0007 D2).
pub fn detect_chat_id(updates: &Value) -> Option<i64> {
    let arr = updates.get("result")?.as_array()?;
    let mut found = None;
    for update in arr {
        let Some(msg) = update.get("message") else {
            continue;
        };
        let text = msg.get("text").and_then(Value::as_str).unwrap_or("");
        if !text.trim_start().starts_with("/start") {
            continue;
        }
        if let Some(id) = msg
            .get("chat")
            .and_then(|c| c.get("id"))
            .and_then(Value::as_i64)
        {
            found = Some(id);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A recording transport returning canned responses, so client requests can
    /// be asserted without a network.
    #[derive(Default)]
    struct FakeTransport {
        /// `(method, body)` of every call, in order. GETs record `Value::Null`.
        calls: RefCell<Vec<(String, Value)>>,
        get_response: Value,
        post_response: Value,
    }

    impl Transport for FakeTransport {
        fn get(&self, method: &str) -> Result<Value> {
            self.calls
                .borrow_mut()
                .push((method.to_string(), Value::Null));
            Ok(self.get_response.clone())
        }

        fn post(&self, method: &str, body: Value) -> Result<Value> {
            self.calls.borrow_mut().push((method.to_string(), body));
            Ok(self.post_response.clone())
        }
    }

    #[test]
    fn send_message_posts_chat_id_and_text() {
        let fake = FakeTransport {
            post_response: json!({ "ok": true, "result": { "message_id": 7 } }),
            ..Default::default()
        };
        let client = BotClient::new(fake);
        let sent = client.send_message(99, "ping").unwrap();
        assert_eq!(sent["message_id"], 7);

        let calls = client.transport.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "sendMessage");
        assert_eq!(calls[0].1["chat_id"], 99);
        assert_eq!(calls[0].1["text"], "ping");
    }

    #[test]
    fn edit_message_text_posts_message_id() {
        let fake = FakeTransport {
            post_response: json!({ "ok": true, "result": {} }),
            ..Default::default()
        };
        let client = BotClient::new(fake);
        client.edit_message_text(99, 7, "updated").unwrap();

        let calls = client.transport.calls.borrow();
        assert_eq!(calls[0].0, "editMessageText");
        assert_eq!(calls[0].1["chat_id"], 99);
        assert_eq!(calls[0].1["message_id"], 7);
        assert_eq!(calls[0].1["text"], "updated");
    }

    #[test]
    fn get_me_parses_ok_result() {
        let fake = FakeTransport {
            get_response: json!({ "ok": true, "result": { "id": 1, "username": "ralphy_bot" } }),
            ..Default::default()
        };
        let client = BotClient::new(fake);
        let me = client.get_me().unwrap();
        assert_eq!(me["username"], "ralphy_bot");
    }

    #[test]
    fn get_me_surfaces_api_error() {
        let fake = FakeTransport {
            get_response: json!({ "ok": false, "description": "Unauthorized" }),
            ..Default::default()
        };
        let client = BotClient::new(fake);
        let err = client.get_me().unwrap_err().to_string();
        assert!(err.contains("Unauthorized"), "got: {err}");
    }

    #[test]
    fn detect_chat_id_reads_start_message_chat() {
        let updates = json!({
            "ok": true,
            "result": [
                { "update_id": 1, "message": { "text": "/start", "chat": { "id": 4242 } } }
            ]
        });
        assert_eq!(detect_chat_id(&updates), Some(4242));
    }

    #[test]
    fn detect_chat_id_takes_last_start_and_skips_non_messages() {
        let updates = json!({
            "ok": true,
            "result": [
                { "update_id": 1, "message": { "text": "/start", "chat": { "id": 1 } } },
                // A non-message update must not abort the scan.
                { "update_id": 2, "edited_message": { "text": "/start", "chat": { "id": 2 } } },
                // A later /start from a different chat wins.
                { "update_id": 3, "message": { "text": "/start@bot", "chat": { "id": 3 } } }
            ]
        });
        assert_eq!(detect_chat_id(&updates), Some(3));
    }

    #[test]
    fn detect_chat_id_returns_none_without_start() {
        let updates = json!({
            "ok": true,
            "result": [
                { "update_id": 1, "message": { "text": "hello", "chat": { "id": 4242 } } }
            ]
        });
        assert_eq!(detect_chat_id(&updates), None);
        // Empty updates also yield None.
        assert_eq!(detect_chat_id(&json!({ "ok": true, "result": [] })), None);
    }
}
