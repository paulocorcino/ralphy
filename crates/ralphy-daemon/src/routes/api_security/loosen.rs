//! The flags that loosen a content policy of the daemon (docs/adr/0032
//! amendment F): each is off by default, and turning one on costs a fresh code.

use axum::extract::Form;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::{apply_auth_change, step_up, store_unavailable};
use crate::auth;
use crate::routes::now_unix;

/// The `POST /api/security/remote-images` body: the desired toggle state, and
/// the current 6-digit code when ENABLING (step-up, amendment E).
#[derive(serde::Deserialize)]
pub(crate) struct RemoteImagesForm {
    pub(crate) enable: bool,
    pub(crate) code: Option<String>,
}

/// `POST /api/security/remote-images`: persist the operator's choice to let
/// rendered markdown load images from other origins (amendment §F). The policy
/// is rebuilt so the next response carries the new CSP; sessions stay, because
/// no credential changed. Turning it ON loosens the CSP, so it costs a fresh
/// code; turning it off stays free. An open page keeps the CSP it loaded with
/// until it reloads.
pub(crate) fn security_remote_images_route(
    state: &auth::AuthState,
    Form(form): Form<RemoteImagesForm>,
) -> Response {
    let dir = match auth::store_dir() {
        Ok(dir) => dir,
        Err(e) => return store_unavailable(&e),
    };
    if form.enable {
        if let Err(refused) =
            step_up::require_fresh_totp(state, &dir, form.code.as_deref(), now_unix())
        {
            return refused.into_response();
        }
    }
    match auth::set_remote_images_in(&dir, form.enable) {
        Ok(()) => {
            apply_auth_change(state, false);
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to write the remote-images flag");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not write the flag",
            )
                .into_response()
        }
    }
}
