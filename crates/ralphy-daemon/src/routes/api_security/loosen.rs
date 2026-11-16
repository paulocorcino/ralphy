//! The flags that loosen a content policy of the daemon (docs/adr/0032
//! amendment F): each is off by default, and turning one on costs a fresh code.

use axum::extract::Form;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::{apply_auth_change, step_up, store_unavailable};
use crate::auth;
use crate::routes::now_unix;

/// The body of a flag that loosens a policy: the desired toggle state, and
/// the current 6-digit code when ENABLING (step-up, amendment E).
#[derive(serde::Deserialize)]
pub(crate) struct LoosenForm {
    pub(crate) enable: bool,
    pub(crate) code: Option<String>,
}

/// A flag that loosens a content policy of the daemon.
#[derive(Clone, Copy)]
pub(crate) enum Loosening {
    /// `POST /api/security/remote-images`: rendered markdown and HTML may load
    /// images from other origins (amendment §F). An open page keeps the CSP
    /// it loaded with until it reloads.
    RemoteImages,
    /// `POST /api/security/web-content`: an HTML file that runs its scripts
    /// may load scripts, styles and fonts from the web (ADR-0089).
    WebContent,
}

/// Persist the operator's choice for one loosening flag. The policy is
/// rebuilt so the next response carries it; sessions stay, because no
/// credential changed. Turning it ON loosens a policy, so it costs a fresh
/// code; turning it off stays free.
pub(crate) fn security_loosen_route(
    state: &auth::AuthState,
    flag: Loosening,
    Form(form): Form<LoosenForm>,
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
    let written = match flag {
        Loosening::RemoteImages => auth::set_remote_images_in(&dir, form.enable),
        Loosening::WebContent => auth::set_web_content_in(&dir, form.enable),
    };
    match written {
        Ok(()) => {
            apply_auth_change(state, false);
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to write a loosening flag");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not write the flag",
            )
                .into_response()
        }
    }
}

/// The `POST` route of one loosening flag.
pub(super) fn loosen_route(
    auth: &std::sync::Arc<auth::AuthState>,
    flag: Loosening,
) -> axum::routing::MethodRouter {
    let auth = std::sync::Arc::clone(auth);
    axum::routing::post(move |form: Form<LoosenForm>| {
        std::future::ready(security_loosen_route(&auth, flag, form))
    })
}
