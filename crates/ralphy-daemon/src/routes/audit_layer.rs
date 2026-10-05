//! The audit layer (ADR-0074): it gives each browser its device cookie, hands
//! the device ID to the handlers as a request extension, and records the
//! login, the logout and every request that changes state. It runs inside the
//! auth guard, so it sees only requests the guard let through.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, Method, StatusCode};
use axum::response::Response;

use crate::audit::{Actor, Audit, Event, EventKind, LoginFailure, ServerFacts};
use crate::device::{self, DeviceId};

/// Mutating paths the layer does not record as an `action`: a peer polls this
/// one every few seconds, so its lines would push every other line out.
const NOT_AN_ACTION: &[&str] = &["/api/peer/tree/poll"];

pub(crate) async fn audit_layer(
    State(audit): State<Arc<Audit>>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = req.uri().path().to_string();
    if !path.starts_with("/api/") && !path.starts_with("/ws") {
        return next.run(req).await;
    }
    let Some(key) = audit.key(path.starts_with("/api/")) else {
        return next.run(req).await;
    };
    let cookie_header = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok());
    let mut new_cookie = None;
    let device = match key.id_in_cookie_header(cookie_header) {
        Some(id) => Some(id),
        // A WebSocket upgrade answers 101, and a browser may drop a cookie set
        // there; the page's first `/api/session` sets it instead.
        None if path.starts_with("/api/") => {
            let id = DeviceId::mint();
            let secure = super::request_is_https(req.headers());
            new_cookie = Some(device::set_cookie_value(&key.sign(id), secure));
            Some(id)
        }
        None => None,
    };
    if let Some(id) = device {
        req.extensions_mut().insert(id);
    }
    let actor = if req.headers().contains_key(header::AUTHORIZATION) {
        Actor::Bearer
    } else if device.is_some() {
        Actor::Device
    } else {
        Actor::Unknown
    };
    let method = req.method().clone();
    let server = ServerFacts::from_headers(req.headers());

    let mut resp = next.run(req).await;

    if let Some(event) = event_for(&method, &path, resp.status(), device, actor, server) {
        audit.record(&event);
    }
    if let Some(set_cookie) = new_cookie {
        if let Ok(v) = header::HeaderValue::from_str(&set_cookie) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    resp
}

/// The line one finished request writes, or `None`.
fn event_for(
    method: &Method,
    path: &str,
    status: StatusCode,
    device: Option<DeviceId>,
    actor: Actor,
    server: ServerFacts,
) -> Option<Event> {
    let mutating = [Method::POST, Method::PUT, Method::PATCH, Method::DELETE].contains(method);
    if !mutating || !path.starts_with("/api/") || NOT_AN_ACTION.contains(&path) {
        return None;
    }
    let session_event = |kind, reason| {
        let mut e = Event::new(kind, device, actor);
        e.reason = reason;
        e.server = Some(server.clone());
        Some(e)
    };
    match (path, status) {
        ("/api/login", StatusCode::OK) => session_event(EventKind::LoginOk, None),
        ("/api/login", StatusCode::UNAUTHORIZED) => {
            session_event(EventKind::LoginFailed, Some(LoginFailure::BadCredential))
        }
        ("/api/login", StatusCode::TOO_MANY_REQUESTS) => {
            session_event(EventKind::LoginFailed, Some(LoginFailure::Throttled))
        }
        // A login on a daemon with no login enabled (404) is not a login.
        ("/api/login", _) => None,
        ("/api/logout", s) if s.is_success() => session_event(EventKind::Logout, None),
        _ => {
            let mut e = Event::new(EventKind::Action, device, actor);
            e.method = Some(method.to_string());
            e.path = Some(path.to_string());
            e.status = Some(status.as_u16());
            e.ip = server.real_ip;
            Some(e)
        }
    }
}
