//! Login, logout and the login card's session state.

use axum::extract::Form;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::routes::{now_unix, request_is_https};
use crate::{auth, cookie};

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
pub(crate) fn login_submit(
    state: &auth::AuthState,
    headers: &axum::http::HeaderMap,
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
    match session.login_checked(
        auth::LoginAttempt {
            code: &form.code,
            password: form.password.as_deref(),
        },
        kind,
        now,
        last_step,
    ) {
        auth::LoginOutcome::Ok {
            cookie,
            kind,
            step,
            login,
        } => {
            // Persist the consumed step so the same code can't be replayed.
            state.record_step(step);
            state.throttle_record(true);
            // `Secure` when the login came through a TLS front (audit F6): the
            // cookie then never rides plain http, and a loopback browser on
            // `http://127.0.0.1` keeps its plain cookie.
            let secure = request_is_https(headers);
            let mut resp = (
                StatusCode::OK,
                [(
                    header::SET_COOKIE,
                    cookie::set_cookie_value_with(&cookie, kind, secure),
                )],
            )
                .into_response();
            // The layers around the route learn which login this response
            // starts.
            resp.extensions_mut().insert(login);
            resp
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
pub(crate) fn logout_route(state: &auth::AuthState, headers: &axum::http::HeaderMap) -> Response {
    if let Err(e) = state.invalidate_sessions() {
        // A failed epoch bump must not strand the operator "logged in": clearing
        // the cookie still drops this browser. Log and proceed.
        tracing::warn!(error = %e, "failed to bump the session epoch on logout");
    }
    let secure = request_is_https(headers);
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
    /// (see [`crate::routes::require_auth`]). An avatar is one emoji drawn from the fixed,
    /// source-visible pool in [`crate::identity::AVATARS`] — it identifies nothing an
    /// attacker did not already have (they are looking at the daemon), while a
    /// name is the operator's own words about their machine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) avatar: Option<String>,
}

/// `GET /api/session`: report whether this request is authorized and whether a
/// password factor is enrolled. `Localhost`/`Bearer` are always `authed`; under a
/// `Session` policy `authed` reflects a valid `Bearer` OR session cookie.
pub(crate) fn session_state_route(
    state: &auth::AuthState,
    avatar: Option<String>,
    headers: &axum::http::HeaderMap,
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
            auth::AuthPolicy::Localhost | auth::AuthPolicy::Bearer(_) => false,
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
