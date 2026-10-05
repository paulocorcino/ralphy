//! `POST /api/device/facts` (ADR-0074 D5): the page reports its device facts
//! once for each load, and the audit log records them.

use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Extension, Json, Router};

use super::RouterShared;
use crate::audit::facts::ClientFacts;
use crate::audit::ServerFacts;
use crate::device::DeviceId;

/// The largest facts body the route reads (ADR-0072 D11). The seven measured
/// devices sent at most 6 KiB in the summarized shape.
pub(crate) const FACTS_MAX_BYTES: usize = 16 * 1024;

pub(crate) fn audit_routes(s: &RouterShared) -> Router {
    let audit = s.audit.clone();
    Router::new()
        .route(
            "/api/device/facts",
            post(
                move |device: Option<Extension<DeviceId>>,
                      headers: HeaderMap,
                      Json(client): Json<ClientFacts>| {
                    let audit = audit.clone();
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
}
