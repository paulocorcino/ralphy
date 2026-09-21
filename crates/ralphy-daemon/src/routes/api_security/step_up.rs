//! Step-up authentication for the operations that LOWER the auth posture
//! (security audit 2026-09-21, F2; docs/adr/0032 amendment E). The ambient
//! session — a cookie that may be a month old, a bearer that may have leaked —
//! is enough to read and to work, not enough to remove a factor, rotate the
//! signing key, lift the login gate or disarm TOTP. Those cost a fresh factor:
//! the current TOTP code (with the login's anti-replay and throttle), or for
//! the password operations the current password.

use std::path::Path;

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::{auth, password, totp};

/// Why a step-up was refused. Small on purpose (a `Response` in an `Err` is
/// what clippy calls a very large variant); it becomes the response at the
/// route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The login throttle is locked out for this many more seconds.
    Throttled(u64),
    /// A live TOTP seed is armed and the body carried no code.
    CodeRequired,
    /// A password is enrolled and the body carried no `current`.
    PasswordRequired,
    /// Wrong code, replayed code, or wrong password — one message, as on login.
    BadCredential,
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        match self {
            Refusal::Throttled(retry_after) => (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, retry_after.to_string())],
                "too many attempts — try again shortly",
            )
                .into_response(),
            Refusal::CodeRequired => {
                (StatusCode::UNAUTHORIZED, "verification code required").into_response()
            }
            Refusal::PasswordRequired => {
                (StatusCode::UNAUTHORIZED, "current password required").into_response()
            }
            Refusal::BadCredential => {
                (StatusCode::UNAUTHORIZED, "invalid credentials").into_response()
            }
        }
    }
}

/// Demand a fresh TOTP `code` when a LIVE seed is armed. The code must match the
/// current step (±1) and be strictly newer than the last consumed one — the same
/// anti-replay as `/api/login` — and every miss feeds the same throttle, so a
/// stolen session cannot brute-force the 6 digits online. With no seed armed
/// there is no factor to spend: first-time setup stays frictionless, and a
/// pending (unconfirmed) enrolment never counts (amendment §C).
pub(super) fn require_fresh_totp(
    state: &auth::AuthState,
    dir: &Path,
    code: Option<&str>,
    now: u64,
) -> Result<(), Refusal> {
    let Some(seed) = totp::load_seed_from(&totp::seed_path_in(dir))
        .ok()
        .flatten()
    else {
        return Ok(());
    };
    state.throttle_check().map_err(Refusal::Throttled)?;
    // An absent code is not a guess, so not a throttle hit: the client simply
    // did not ask for the factor.
    let code = code
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .ok_or(Refusal::CodeRequired)?;
    let fresh = seed
        .matched_step(code, now, 1)
        .filter(|step| state.last_step().is_none_or(|last| *step > last));
    match fresh {
        Some(step) => {
            state.record_step(step);
            state.throttle_record(true);
            Ok(())
        }
        None => {
            state.throttle_record(false);
            Err(Refusal::BadCredential)
        }
    }
}

/// Demand the `current` password when one is enrolled under `dir`. Verified
/// against the STORE (not the live policy): under a `Localhost` policy the
/// password is enrolled but gates nothing, and it must still be the thing that
/// authorizes its own replacement. Misses feed the login throttle — PBKDF2 makes
/// each guess slow, the throttle makes them few.
pub(super) fn require_current_password(
    state: &auth::AuthState,
    dir: &Path,
    current: Option<&str>,
) -> Result<(), Refusal> {
    let Some(enrolled) = password::load_from(&password::password_path_in(dir))
        .ok()
        .flatten()
    else {
        return Ok(());
    };
    state.throttle_check().map_err(Refusal::Throttled)?;
    let current = current.ok_or(Refusal::PasswordRequired)?;
    if enrolled.verify(current) {
        state.throttle_record(true);
        Ok(())
    } else {
        state.throttle_record(false);
        Err(Refusal::BadCredential)
    }
}
