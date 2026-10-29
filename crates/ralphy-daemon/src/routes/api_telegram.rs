//! `GET /api/telegram`: the Telegram state of this daemon (docs/adr/0085
//! D14), read from its config at each request. The attention panel shows it.

use std::path::PathBuf;

use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use super::RouterShared;
use crate::telegram_state;

/// `GET /api/telegram` → `{"state": "off" | "sends"}`. The config is read off
/// the runtime: it is a file.
pub(crate) async fn telegram_route(config: PathBuf) -> Response {
    match super::blocking_read(move || telegram_state::state_at(&config)).await {
        Some(state) => Json(state).into_response(),
        None => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "the Telegram config could not be read",
        )
            .into_response(),
    }
}

/// `/api/telegram`, behind the same guard as every route.
pub(crate) fn telegram_routes(s: &RouterShared) -> Router {
    let config = s.stores.telegram_config.clone();
    Router::new().route("/api/telegram", get(move || telegram_route(config.clone())))
}

#[cfg(test)]
mod tests;
