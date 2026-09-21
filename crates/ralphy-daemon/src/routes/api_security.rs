//! Login, logout, the session state and `/api/security/*` (docs/adr/0032
//! amendment): every mutation of the auth posture and the `*_at` store
//! helpers behind it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use axum::extract::Form;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::now_unix;
use crate::{auth, cookie, password, release, totp};

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
            (
                StatusCode::OK,
                [(header::SET_COOKIE, cookie::set_cookie_value(&cookie, kind))],
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
pub(crate) async fn logout_route(state: Arc<auth::AuthState>) -> Response {
    if let Err(e) = state.invalidate_sessions() {
        // A failed epoch bump must not strand the operator "logged in": clearing
        // the cookie still drops this browser. Log and proceed.
        tracing::warn!(error = %e, "failed to bump the session epoch on logout");
    }
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie::clear_cookie_value())],
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
    /// (see [`require_auth`]). An avatar is one emoji drawn from the fixed,
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

/// The daemon's auth-state surface for the Security modal (issue #195): which
/// factors are enrolled in the REAL stores. `require_login` is the PERSISTED
/// `daemon-require-login` flag (ADR-0032 amendment §A): an operator opt-in that
/// gates the browser UI even on a loopback bind, no longer derived from the seed.
#[derive(serde::Serialize)]
pub(crate) struct SecurityState {
    pub(crate) token_set: bool,
    pub(crate) password_set: bool,
    pub(crate) totp_enrolled: bool,
    pub(crate) require_login: bool,
}

/// Read the real store FILES under `dir` and report enrolment. Path-explicit (no
/// env reads) so tests pass a tempdir. `require_login` is now the PERSISTED flag
/// (ADR-0032 amendment §A), no longer derived from the seed.
pub(crate) fn security_state_at(dir: &Path) -> SecurityState {
    let totp_enrolled = totp::load_seed_from(&totp::seed_path_in(dir))
        .ok()
        .flatten()
        .is_some();
    SecurityState {
        token_set: auth::load_token_from(&auth::token_path_in(dir))
            .ok()
            .flatten()
            .is_some(),
        password_set: password::load_from(&password::password_path_in(dir))
            .ok()
            .flatten()
            .is_some(),
        totp_enrolled,
        require_login: auth::require_login_enabled_in(dir),
    }
}

/// Begin enrolment: mint-once a PENDING TOTP seed under `dir` and return its
/// `otpauth://` URI + whether it was newly minted. The seed is NOT armed — it
/// gates nothing until [`confirm_totp_at`] verifies a code (ADR-0032 amendment
/// §C). The URI is shown once (QR + base32); a re-enrol before confirming
/// returns the SAME pending secret with `newly_minted=false`.
pub(crate) fn enroll_totp_at(dir: &Path) -> Result<(String, bool)> {
    let (seed, newly_minted) = totp::ensure_seed_at(&totp::pending_seed_path_in(dir))?;
    Ok((seed.otpauth_uri("ralphy", "daemon"), newly_minted))
}

/// Confirm a pending enrolment: verify `code` against the pending seed and, on
/// success, promote it to the live seed. Returns whether the code verified.
pub(crate) fn confirm_totp_at(dir: &Path, code: &str, now: u64) -> Result<bool> {
    totp::confirm_pending_at(
        &totp::pending_seed_path_in(dir),
        &totp::seed_path_in(dir),
        code,
        now,
    )
}

/// Revoke enrolment: delete the live seed, any in-flight pending seed, and the
/// anti-replay last-step marker, so an abandoned enrolment leaves nothing behind
/// (ADR-0032 amendment §C/§D).
pub(crate) fn revoke_totp_at(dir: &Path) -> Result<()> {
    totp::revoke_seed_at(&totp::seed_path_in(dir))?;
    totp::revoke_seed_at(&totp::pending_seed_path_in(dir))?;
    totp::revoke_seed_at(&totp::last_step_path_in(dir))
}

/// Set (non-empty) or clear (empty/absent) the optional password under `dir`;
/// return whether a password is now enrolled.
pub(crate) fn set_password_at(dir: &Path, password: Option<&str>) -> Result<bool> {
    let path = password::password_path_in(dir);
    match password.filter(|p| !p.is_empty()) {
        Some(pw) => {
            password::save_to(&password::Hash::hash_password(pw), &path)?;
            Ok(true)
        }
        None => {
            password::clear_at(&path)?;
            Ok(false)
        }
    }
}

/// Remint the access token under `dir`, overwriting any prior. The token is
/// never echoed — only its rotation is reported.
pub(crate) fn remint_token_at(dir: &Path) -> Result<()> {
    auth::save_token_to(&auth::generate_token(), &auth::token_path_in(dir))
}

/// The require-login gate (ADR-0032 amendment §A): persist the operator's choice
/// as the `daemon-require-login` flag. Enabling demands an armed TOTP seed
/// (`Err("totp not enrolled")` otherwise) and MINTS the access token if absent —
/// gating a loopback bind needs a signing key, and machine clients then use it as
/// a bearer. Disabling just clears the flag.
pub(crate) fn require_login_at(dir: &Path, enable: bool) -> Result<()> {
    if enable {
        if totp::load_seed_from(&totp::seed_path_in(dir))
            .ok()
            .flatten()
            .is_none()
        {
            anyhow::bail!("totp not enrolled");
        }
        // Ensure a signing key exists (mint-once) so the gate can sign cookies —
        // a loopback bind may never have minted one.
        auth::ensure_token_at(&auth::token_path_in(dir))?;
    }
    auth::set_require_login_in(dir, enable)
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

/// `POST /api/security/totp/revoke`: delete the live AND pending seeds (mint-once
/// posture). Rebuilds the policy (demoting a gated bind) and invalidates live
/// sessions — revoking the factor must drop anyone it authorized.
pub(crate) async fn security_totp_revoke_route(state: Arc<auth::AuthState>) -> Response {
    match auth::store_dir().and_then(|dir| revoke_totp_at(&dir)) {
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
/// empty/absent one clears it.
#[derive(serde::Deserialize)]
pub(crate) struct PasswordForm {
    pub(crate) password: Option<String>,
}

/// `POST /api/security/password`: set or clear the optional password factor.
/// Rebuilds the policy (the `Session` carries the new/absent password) and
/// invalidates live sessions so they re-authenticate under the changed factor.
pub(crate) async fn security_password_route(
    state: Arc<auth::AuthState>,
    Form(form): Form<PasswordForm>,
) -> Response {
    match auth::store_dir().and_then(|dir| set_password_at(&dir, form.password.as_deref())) {
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

/// `POST /api/security/token/remint`: rotate the access token (never echoed).
/// The token is the cookie signing key, so rebuild the policy under the new key
/// and invalidate live sessions — a re-mint logs everyone out (amendment §B),
/// now IMMEDIATELY rather than at next restart.
pub(crate) async fn security_token_remint_route(state: Arc<auth::AuthState>) -> Response {
    match auth::store_dir().and_then(|dir| remint_token_at(&dir)) {
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

/// The `POST /api/security/require-login` body: the desired toggle state.
#[derive(serde::Deserialize)]
pub(crate) struct RequireLoginForm {
    pub(crate) enable: bool,
}

/// `POST /api/security/require-login`: persist the operator's gate choice
/// (amendment §A). Enabling without an armed TOTP seed is refused (`400`, AC4);
/// enabling mints a signing token if absent. Either way the policy is rebuilt so
/// the gate engages/lifts IMMEDIATELY, and live sessions are invalidated (turning
/// the gate on logs the browser off; turning it off re-issues cleanly).
pub(crate) async fn security_require_login_route(
    state: Arc<auth::AuthState>,
    Form(form): Form<RequireLoginForm>,
) -> Response {
    match auth::store_dir().and_then(|dir| require_login_at(&dir, form.enable)) {
        Ok(()) => {
            apply_auth_change(&state, true);
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
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
