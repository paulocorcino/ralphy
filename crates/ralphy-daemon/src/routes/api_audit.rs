//! The audit log's routes (ADR-0074): the page reports its device facts once
//! for each load (D5), the Devices section reads the log back (D9), and the
//! page reads and dismisses the login notices (#717).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Form, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde_json::json;

use super::RouterShared;
use crate::audit::facts::ClientFacts;
use crate::audit::{logins, read, Actor, Audit};
use crate::auth::AuthState;
use crate::cookie::LoginId;
use crate::device::DeviceId;
use crate::registry;

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

/// `POST /api/device/facts`: record the device facts the page reports.
async fn post_facts(
    audit: Arc<Audit>,
    caller: Option<Extension<super::audit_layer::Caller>>,
    client: ClientFacts,
) -> StatusCode {
    // No device yet: the page's first `/api/session` has
    // not answered. The page sends again (D5).
    let Some((device, server)) = caller.and_then(|Extension(c)| Some((c.device?, c.server))) else {
        return StatusCode::CONFLICT;
    };
    // The file write is short; it runs off the async
    // workers like the other store writes.
    let written =
        tokio::task::spawn_blocking(move || audit.record_facts(device, client, server)).await;
    if let Err(e) = written {
        tracing::warn!(error = %e, "recording device facts failed");
    }
    StatusCode::NO_CONTENT
}

/// `GET /api/audit/devices`: the devices of the log, the asking one marked.
async fn get_devices(audit: Arc<Audit>, device: Option<Extension<DeviceId>>) -> Response {
    read_reply("devices", move || match audit.log_path() {
        Some(path) => read::devices(&path, device.map(|Extension(id)| id)),
        None => Ok(Vec::new()),
    })
    .await
}

/// `GET /api/audit/events`: the events of one device, with project names.
async fn get_events(audit: Arc<Audit>, registry_path: PathBuf, q: EventsQuery) -> Response {
    if !is_device_id(&q.device) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "the device is not a device ID" })),
        )
            .into_response();
    }
    let limit = q.limit.unwrap_or(read::MAX_EVENTS);
    read_reply("events", move || {
        let Some(path) = audit.log_path() else {
            return Ok(Vec::new());
        };
        let mut events = read::events(&path, &q.device, limit)?;
        // The names are a help to read the lines: an unreadable
        // registry shows the recorded values instead.
        match registry::load_from(&registry_path) {
            Ok(store) => read::name_repos(&mut events, &store),
            Err(e) => tracing::warn!(
                error = %format!("{e:#}"),
                "the audit events are shown without project names"
            ),
        }
        Ok(events)
    })
    .await
}

/// `GET /api/audit/logins/notices`: the open login notices, the newest
/// first.
async fn get_notices(
    audit: Arc<Audit>,
    auth: Arc<AuthState>,
    device: Option<Extension<DeviceId>>,
) -> Response {
    read_reply("notices", move || {
        let Some(path) = audit.log_path() else {
            return Ok(Vec::new());
        };
        let revoked = |login: &str| LoginId::parse(login).is_some_and(|id| auth.login_revoked(&id));
        logins::notices(
            &path,
            chrono::Utc::now().timestamp(),
            device.map(|Extension(id)| id),
            revoked,
        )
    })
    .await
}

/// The `POST /api/audit/logins/dismiss` body: the pair of the notice. An
/// empty `device` is the use that came with no device cookie.
#[derive(serde::Deserialize)]
pub(crate) struct DismissForm {
    login: Option<String>,
    device: Option<String>,
}

/// The pair of a dismiss body, or `None` when it is malformed.
fn dismiss_pair(form: &DismissForm) -> Option<(LoginId, Option<&str>)> {
    let login = LoginId::parse(form.login.as_deref()?)?;
    match form.device.as_deref()? {
        "" => Some((login, None)),
        device if is_device_id(device) => Some((login, Some(device))),
        _ => None,
    }
}

/// `POST /api/audit/logins/dismiss`: the operator says a notice was their own
/// use. A malformed pair is a `400`; a failed write is a `500`.
async fn post_dismiss(
    audit: Arc<Audit>,
    caller: Option<Extension<super::audit_layer::Caller>>,
    form: DismissForm,
) -> Response {
    let Some((login, device)) = dismiss_pair(&form) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "the notice is not a login ID and a device ID" })),
        )
            .into_response();
    };
    let device = device.map(str::to_string);
    let actor = caller.map_or(Actor::Unknown, |Extension(c)| c.actor);
    let written =
        tokio::task::spawn_blocking(move || audit.dismiss_notice(&login, device.as_deref(), actor))
            .await;
    match written {
        Ok(Ok(())) => Json(json!({ "dismissed": true })).into_response(),
        Ok(Err(e)) => {
            tracing::warn!(error = %format!("{e:#}"), "dismissing a login notice failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "the dismissal was not written" })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("the write stopped: {e}") })),
        )
            .into_response(),
    }
}

pub(crate) fn audit_routes(s: &RouterShared) -> Router {
    let facts_audit = s.audit.clone();
    let devices_audit = s.audit.clone();
    let events_audit = s.audit.clone();
    let notices_audit = s.audit.clone();
    let notices_auth = s.auth.clone();
    let dismiss_audit = s.audit.clone();
    let registry_path = s.registry_path.clone();
    Router::new()
        .route(
            "/api/device/facts",
            post(
                move |caller: Option<Extension<super::audit_layer::Caller>>,
                      Json(client): Json<ClientFacts>| {
                    post_facts(facts_audit.clone(), caller, client)
                },
            ),
        )
        .layer(DefaultBodyLimit::max(FACTS_MAX_BYTES))
        .route(
            "/api/audit/devices",
            get(move |device: Option<Extension<DeviceId>>| {
                get_devices(devices_audit.clone(), device)
            }),
        )
        .route(
            "/api/audit/events",
            get(move |Query(q): Query<EventsQuery>| {
                get_events(events_audit.clone(), registry_path.clone(), q)
            }),
        )
        .route(
            "/api/audit/logins/notices",
            get(move |device: Option<Extension<DeviceId>>| {
                get_notices(notices_audit.clone(), notices_auth.clone(), device)
            }),
        )
        .route(
            "/api/audit/logins/dismiss",
            post(
                move |caller: Option<Extension<super::audit_layer::Caller>>,
                      Form(form): Form<DismissForm>| {
                    post_dismiss(dismiss_audit.clone(), caller, form)
                },
            ),
        )
}
