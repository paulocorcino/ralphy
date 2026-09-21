//! Login, logout, the session state and `/api/security/*` (docs/adr/0032
//! amendment): every mutation of the auth posture. The `*_at` store helpers
//! behind them live in [`store`], the step-up factor checks in [`step_up`].

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::Form;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::now_unix;
use crate::{auth, cookie, release};

mod step_up;
mod store;

pub(crate) use store::*;

/// The `POST /api/login` form: the current TOTP `code` and, when a password is
/// enrolled, the operator's `password`. `password` is `Option` so a bind with no
/// password enrolled accepts a form carrying only `code`. `remember` is the
/// "keep me signed in" box (ADR-0032 amendment 2026-09-16): absent or `false`
/// mints a standard session, `true` a remembered one.
#[derive(serde::Deserialize)]
pub(crate) struct LoginForm {
    pub(crate) code: String,
    pub(crate) password: Option<String>,
    #[serde(default)]
    pub(crate) remember: bool,
}

/// `POST /api/login`: validate the TOTP code (and password, if enrolled) against
/// the CURRENT `Session` policy. On success `200` + a `Set-Cookie: ralphy_session=…`
/// header; on a bad credential `401`; while rate-limited `429` with a
/// `Retry-After` (amendment §D). Login is meaningless without a `Session` policy,
/// so any other policy returns `404`.
pub(crate) async fn login_submit(
    state: Arc<auth::AuthState>,
    headers: axum::http::HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    // Throttle first: a 6-digit TOTP is otherwise online-brute-forceable.
    if let Err(retry_after) = state.throttle_check() {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after.to_string())],
            "too many attempts — try again shortly",
        )
            .into_response();
    }
    let auth::AuthPolicy::Session(session) = state.policy() else {
        return (StatusCode::NOT_FOUND, "login not enabled").into_response();
    };
    let now = now_unix();
    // The last consumed TOTP step gates anti-replay (amendment §D); the store lives
    // on the AuthState (real store at boot, detached temp in tests).
    let last_step = state.last_step();
    let kind = if form.remember {
        cookie::SessionKind::Remembered
    } else {
        cookie::SessionKind::Standard
    };
    match session.login_checked(&form.code, form.password.as_deref(), kind, now, last_step) {
        auth::LoginOutcome::Ok { cookie, kind, step } => {
            // Persist the consumed step so the same code can't be replayed.
            state.record_step(step);
            state.throttle_record(true);
            // `Secure` when the login came through a TLS front (audit F6): the
            // cookie then never rides plain http, and a loopback browser on
            // `http://127.0.0.1` keeps its plain cookie.
            let secure = super::request_is_https(&headers);
            (
                StatusCode::OK,
                [(
                    header::SET_COOKIE,
                    cookie::set_cookie_value_with(&cookie, kind, secure),
                )],
            )
                .into_response()
        }
        // A replay and a bad credential are indistinguishable to the client (generic
        // message) and both feed the throttle.
        auth::LoginOutcome::BadCredential | auth::LoginOutcome::Replayed => {
            state.throttle_record(false);
            (StatusCode::UNAUTHORIZED, "invalid credentials").into_response()
        }
    }
}

/// `POST /api/logout`: bump the session epoch so the cookie is invalidated
/// SERVER-SIDE (amendment §B — not merely cleared client-side), then emit a
/// `Max-Age=0` clearing `Set-Cookie`. The cookie is `HttpOnly`, so JS cannot
/// clear it — the server must (issue #186).
pub(crate) async fn logout_route(
    state: Arc<auth::AuthState>,
    headers: axum::http::HeaderMap,
) -> Response {
    if let Err(e) = state.invalidate_sessions() {
        // A failed epoch bump must not strand the operator "logged in": clearing
        // the cookie still drops this browser. Log and proceed.
        tracing::warn!(error = %e, "failed to bump the session epoch on logout");
    }
    let secure = super::request_is_https(&headers);
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie::clear_cookie_value_with(secure))],
    )
        .into_response()
}

/// The SPA's auth-state oracle, reachable pre-login (allowlisted). Drives the
/// workbench gate's `authed` flag, password-field visibility, and the Security
/// modal's policy-aware affordances (issue #205).
#[derive(serde::Serialize)]
pub(crate) struct SessionState {
    pub(crate) authed: bool,
    pub(crate) password: bool,
    pub(crate) policy: &'static str,
    /// The daemon's avatar, so the login card wears THIS daemon's face instead of
    /// a generic robot — an operator with several daemons open has nothing else
    /// on that screen to tell them apart by.
    ///
    /// The NAME is deliberately not here. This route is pre-login: everything on
    /// it is readable without a cookie, and the login gate is meant to be opaque
    /// (see [`super::require_auth`]). An avatar is one emoji drawn from the fixed,
    /// source-visible pool in [`identity::AVATARS`] — it identifies nothing an
    /// attacker did not already have (they are looking at the daemon), while a
    /// name is the operator's own words about their machine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) avatar: Option<String>,
}

/// `GET /api/session`: report whether this request is authorized and whether a
/// password factor is enrolled. `Localhost`/`Bearer` are always `authed`; under a
/// `Session` policy `authed` reflects a valid `Bearer` OR session cookie.
pub(crate) async fn session_state_route(
    state: Arc<auth::AuthState>,
    avatar: Option<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    let auth = state.policy();
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let authed = auth.authorizes(bearer)
        || match &auth {
            auth::AuthPolicy::Session(s) => {
                let cookie_header = headers.get(header::COOKIE).and_then(|v| v.to_str().ok());
                s.cookie_valid(cookie_header, now_unix())
            }
            _ => false,
        };
    let password = matches!(&auth, auth::AuthPolicy::Session(s) if s.password.is_some());
    Json(SessionState {
        authed,
        password,
        policy: auth.name(),
        avatar,
    })
    .into_response()
}

/// `GET /api/security/state`: the real enrolment state (gated by `require_auth`;
/// not in the login allowlist).
pub(crate) async fn security_state_route() -> Response {
    match auth::store_dir() {
        Ok(dir) => Json(security_state_at(&dir)).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "failed to resolve the daemon store for security state");
            (StatusCode::INTERNAL_SERVER_ERROR, "store unavailable").into_response()
        }
    }
}

/// `POST /api/security/totp/enroll`: mint-once the seed and return the one-time
/// `otpauth://` URI + `newly_minted`.
pub(crate) async fn security_totp_enroll_route() -> Response {
    match auth::store_dir().and_then(|dir| enroll_totp_at(&dir)) {
        Ok((uri, newly_minted)) => {
            Json(serde_json::json!({ "uri": uri, "newly_minted": newly_minted })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to enroll a TOTP seed");
            (StatusCode::INTERNAL_SERVER_ERROR, "enroll failed").into_response()
        }
    }
}

/// The `POST /api/security/totp/confirm` body: the live 6-digit code proving the
/// operator scanned the pending QR.
#[derive(serde::Deserialize)]
pub(crate) struct ConfirmForm {
    pub(crate) code: String,
}

/// `POST /api/security/totp/confirm`: verify `code` against the pending seed and
/// arm it on success. `200 {confirmed}` either way; a wrong code is
/// `confirmed:false` (not an error) so the UI can prompt a retry. On a successful
/// arm the live policy is rebuilt (a promotion takes effect if require-login is
/// already on).
pub(crate) async fn security_totp_confirm_route(
    state: Arc<auth::AuthState>,
    Form(form): Form<ConfirmForm>,
) -> Response {
    let now = now_unix();
    match auth::store_dir().and_then(|dir| confirm_totp_at(&dir, &form.code, now)) {
        Ok(confirmed) => {
            if confirmed {
                apply_auth_change(&state, false);
            }
            Json(serde_json::json!({ "confirmed": confirmed })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to confirm the TOTP enrolment");
            (StatusCode::INTERNAL_SERVER_ERROR, "confirm failed").into_response()
        }
    }
}

/// The `POST /api/security/totp/revoke` body: the current 6-digit code when a
/// live seed is armed (step-up, amendment E); nothing while only a pending
/// enrolment exists (cancelling one costs nothing — it never gated anything).
#[derive(serde::Deserialize)]
pub(crate) struct RevokeForm {
    pub(crate) code: Option<String>,
}

/// `POST /api/security/totp/revoke`: delete the live AND pending seeds (mint-once
/// posture). Rebuilds the policy (demoting a gated bind) and invalidates live
/// sessions — revoking the factor must drop anyone it authorized. On a loopback
/// bind with require-login this is the whole gate going away
/// (`compute_policy` falls back to `Localhost` without a seed), so it costs a
/// fresh code. Lost the authenticator? Delete `daemon-totp` in the store on
/// the host — the operator's own machine is the recovery path, not this route.
pub(crate) async fn security_totp_revoke_route(
    state: Arc<auth::AuthState>,
    Form(form): Form<RevokeForm>,
) -> Response {
    let dir = match auth::store_dir() {
        Ok(dir) => dir,
        Err(e) => return store_unavailable(e),
    };
    if let Err(refused) =
        step_up::require_fresh_totp(&state, &dir, form.code.as_deref(), now_unix())
    {
        return refused.into_response();
    }
    match revoke_totp_at(&dir) {
        Ok(()) => {
            apply_auth_change(&state, true);
            Json(serde_json::json!({ "revoked": true })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to revoke the TOTP seed");
            (StatusCode::INTERNAL_SERVER_ERROR, "revoke failed").into_response()
        }
    }
}

/// The `POST /api/security/password` body: a non-empty `password` sets it, an
/// EMPTY one clears it, and an ABSENT one is a `400` — the field is the
/// operator's explicit intent, and a body that forgot it must not silently
/// remove the factor (amendment E). `current` is the enrolled password, demanded
/// whenever one exists.
#[derive(serde::Deserialize)]
pub(crate) struct PasswordForm {
    pub(crate) password: Option<String>,
    pub(crate) current: Option<String>,
}

/// `POST /api/security/password`: set or clear the optional password factor.
/// Rebuilds the policy (the `Session` carries the new/absent password) and
/// invalidates live sessions so they re-authenticate under the changed factor.
/// Changing or clearing an enrolled password costs the current one; the
/// first-time set is free (bootstrap).
pub(crate) async fn security_password_route(
    state: Arc<auth::AuthState>,
    Form(form): Form<PasswordForm>,
) -> Response {
    let Some(password) = form.password.as_deref() else {
        return (StatusCode::BAD_REQUEST, "password field required").into_response();
    };
    let dir = match auth::store_dir() {
        Ok(dir) => dir,
        Err(e) => return store_unavailable(e),
    };
    if let Err(refused) = step_up::require_current_password(&state, &dir, form.current.as_deref()) {
        return refused.into_response();
    }
    match set_password_at(&dir, Some(password)) {
        Ok(password_set) => {
            apply_auth_change(&state, true);
            Json(serde_json::json!({ "password_set": password_set })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to update the password");
            (StatusCode::INTERNAL_SERVER_ERROR, "password update failed").into_response()
        }
    }
}

/// The `POST /api/security/token/remint` body: the current 6-digit code when a
/// live seed is armed (step-up, amendment E).
#[derive(serde::Deserialize)]
pub(crate) struct RemintForm {
    pub(crate) code: Option<String>,
}

/// `POST /api/security/token/remint`: rotate the access token (never echoed).
/// The token is the cookie signing key, so rebuild the policy under the new key
/// and invalidate live sessions — a re-mint logs everyone out (amendment §B),
/// now IMMEDIATELY rather than at next restart. Costs a fresh code when TOTP is
/// armed: an ambient session must not be able to rotate the key it rides on.
pub(crate) async fn security_token_remint_route(
    state: Arc<auth::AuthState>,
    Form(form): Form<RemintForm>,
) -> Response {
    let dir = match auth::store_dir() {
        Ok(dir) => dir,
        Err(e) => return store_unavailable(e),
    };
    if let Err(refused) =
        step_up::require_fresh_totp(&state, &dir, form.code.as_deref(), now_unix())
    {
        return refused.into_response();
    }
    match remint_token_at(&dir) {
        Ok(()) => {
            apply_auth_change(&state, true);
            Json(serde_json::json!({ "reminted": true })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to remint the access token");
            (StatusCode::INTERNAL_SERVER_ERROR, "remint failed").into_response()
        }
    }
}

/// The `POST /api/release/watch` body: the desired watch state.
#[derive(serde::Deserialize)]
pub(crate) struct ReleaseWatchForm {
    pub(crate) enable: bool,
}

/// `POST /api/release/watch`: turn the release watch on or off.
///
/// Writes the marker the poll reads, so the answer takes effect on the next
/// tick without a restart. Idempotent both ways, like the require-login toggle
/// it is modelled on.
pub(crate) async fn release_watch_route(
    store: Option<PathBuf>,
    Form(form): Form<ReleaseWatchForm>,
) -> Response {
    let Some(dir) = store else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "store unavailable").into_response();
    };
    match release::set_watch_disabled_in(&dir, !form.enable) {
        Ok(()) => Json(serde_json::json!({ "enabled": form.enable })).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "failed to set the release watch state");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not write the flag",
            )
                .into_response()
        }
    }
}

/// The `POST /api/security/require-login` body: the desired toggle state, and
/// the current 6-digit code when DISABLING (step-up, amendment E).
#[derive(serde::Deserialize)]
pub(crate) struct RequireLoginForm {
    pub(crate) enable: bool,
    pub(crate) code: Option<String>,
}

/// `POST /api/security/require-login`: persist the operator's gate choice
/// (amendment §A). Enabling without an armed TOTP seed is refused (`400`, AC4);
/// enabling mints a signing token if absent. Either way the policy is rebuilt so
/// the gate engages/lifts IMMEDIATELY, and live sessions are invalidated (turning
/// the gate on logs the browser off; turning it off re-issues cleanly). Turning
/// it OFF costs a fresh code — that is the gate itself being lowered; turning it
/// on stays free.
pub(crate) async fn security_require_login_route(
    state: Arc<auth::AuthState>,
    Form(form): Form<RequireLoginForm>,
) -> Response {
    let dir = match auth::store_dir() {
        Ok(dir) => dir,
        Err(e) => return store_unavailable(e),
    };
    if !form.enable {
        if let Err(refused) =
            step_up::require_fresh_totp(&state, &dir, form.code.as_deref(), now_unix())
        {
            return refused.into_response();
        }
    }
    match require_login_at(&dir, form.enable) {
        Ok(()) => {
            apply_auth_change(&state, true);
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// The one answer to a store that cannot be resolved, shared by every mutation
/// that needs the directory BEFORE it can decide anything.
fn store_unavailable(e: anyhow::Error) -> Response {
    tracing::warn!(error = %e, "failed to resolve the daemon store");
    (StatusCode::INTERNAL_SERVER_ERROR, "store unavailable").into_response()
}

/// Apply a security mutation to the LIVE auth state: rebuild the policy from disk
/// so the change takes effect without a restart (amendment §A), and — when
/// `invalidate` — bump the session epoch to drop outstanding cookies (§B).
/// Failures are logged, never fatal to the request that triggered them (the store
/// write already succeeded; a stale in-memory policy self-heals on restart).
pub(crate) fn apply_auth_change(state: &auth::AuthState, invalidate: bool) {
    if let Err(e) = state.rebuild() {
        tracing::warn!(error = %e, "failed to rebuild the live auth policy");
    }
    if invalidate {
        if let Err(e) = state.invalidate_sessions() {
            tracing::warn!(error = %e, "failed to invalidate sessions");
        }
    }
}
