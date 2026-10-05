//! The audit log's routes (ADR-0074): the page reports its device facts once
//! for each load (D5), and the Devices section reads the log back (D9).

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde_json::json;

use super::RouterShared;
use crate::audit::facts::ClientFacts;
use crate::audit::{read, Audit, ServerFacts};
use crate::device::DeviceId;

/// The largest facts body the route reads (ADR-0072 D11). The seven measured
/// devices sent at most 6 KiB in the summarized shape.
pub(crate) const FACTS_MAX_BYTES: usize = 16 * 1024;

#[derive(serde::Deserialize)]
pub(crate) struct EventsQuery {
    device: String,
    limit: Option<usize>,
}

/// Run a read of the log off the async workers, as a JSON reply under `key`.
async fn read_reply<T: serde::Serialize + Send + 'static>(
    key: &'static str,
    read: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(read).await {
        Ok(Ok(value)) => Json(json!({ key: value })).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("{e:#}") })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("the read stopped: {e}") })),
        )
            .into_response(),
    }
}

fn is_device_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub(crate) fn audit_routes(s: &RouterShared) -> Router {
    let facts_audit = s.audit.clone();
    let devices_audit = s.audit.clone();
    let events_audit = s.audit.clone();
    Router::new()
        .route(
            "/api/device/facts",
            post(
                move |device: Option<Extension<DeviceId>>,
                      headers: HeaderMap,
                      Json(client): Json<ClientFacts>| {
                    let audit = facts_audit.clone();
                    async move {
                        let server = ServerFacts::from_headers(&headers);
                        // The file write is short; it runs off the async
                        // workers like the other store writes.
                        let written = tokio::task::spawn_blocking(move || {
                            audit.record_facts(device.map(|Extension(id)| id), client, server)
                        })
                        .await;
                        if let Err(e) = written {
                            tracing::warn!(error = %e, "recording device facts failed");
                        }
                        StatusCode::NO_CONTENT
                    }
                },
            ),
        )
        .layer(DefaultBodyLimit::max(FACTS_MAX_BYTES))
        .route(
            "/api/audit/devices",
            get(move |device: Option<Extension<DeviceId>>| {
                let audit: Arc<Audit> = devices_audit.clone();
                async move {
                    read_reply("devices", move || match audit.log_path() {
                        Some(path) => read::devices(&path, device.map(|Extension(id)| id)),
                        None => Ok(Vec::new()),
                    })
                    .await
                }
            }),
        )
        .route(
            "/api/audit/events",
            get(move |Query(q): Query<EventsQuery>| {
                let audit: Arc<Audit> = events_audit.clone();
                async move {
                    if !is_device_id(&q.device) {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "error": "the device is not a device ID" })),
                        )
                            .into_response();
                    }
                    let limit = q.limit.unwrap_or(read::MAX_EVENTS);
                    read_reply("events", move || match audit.log_path() {
                        Some(path) => read::events(&path, &q.device, limit),
                        None => Ok(Vec::new()),
                    })
                    .await
                }
            }),
        )
}
