//! The browser-session credentials of a hardened network bind and their checks.

use super::{LoginOutcome, RevokedLogins};
use crate::{cookie, epoch, password, totp};

/// The browser-session credentials for a hardened network bind (issue #179): the
/// signing-key token, the enrolled TOTP seed, and an OPTIONAL password. Held
/// behind an `Arc` in [`AuthPolicy::Session`](super::AuthPolicy::Session) so
/// the policy stays cheap to clone.
pub struct SessionAuth {
    /// The daemon access token — doubles as the machine bearer AND the cookie
    /// signing key. One secret, two roles (ADR-0032 §4, stateless-cookie).
    pub token: String,
    /// The enrolled TOTP seed (the core login factor).
    pub totp: totp::Seed,
    /// An optional password (defense-in-depth); `None` when the operator did not
    /// enrol one.
    pub password: Option<password::Hash>,
    /// The live session epoch mixed into every cookie (ADR-0032 amendment §B):
    /// bumping it invalidates all outstanding cookies at once.
    pub epoch: epoch::SessionEpoch,
    /// The revoked-login list, shared with the [`AuthState`](super::AuthState)
    /// that revokes: a cookie of a login on it is refused.
    pub revoked: RevokedLogins,
}

impl SessionAuth {
    /// Whether a `Cookie:` header carries a valid, unexpired session cookie
    /// signed by this daemon's token AT THE CURRENT EPOCH.
    pub fn cookie_valid(&self, cookie_header: Option<&str>, now: u64) -> bool {
        self.cookie_login(cookie_header, now).is_some()
    }

    /// The login ID of a valid, unexpired session cookie in a `Cookie:` header,
    /// signed by this daemon's token at the current epoch, of a login that was
    /// not revoked; `None` otherwise.
    pub fn cookie_login(&self, cookie_header: Option<&str>, now: u64) -> Option<cookie::LoginId> {
        self.live_claims(cookie_header, now).map(|c| c.login)
    }

    /// The claims of a cookie that [`cookie_login`](Self::cookie_login) accepts.
    fn live_claims(&self, cookie_header: Option<&str>, now: u64) -> Option<cookie::Claims> {
        let value = cookie::from_cookie_header(cookie_header)?;
        let claims = cookie::verify_claims(&self.token, self.epoch.get(), &value, now)?;
        (!self.revoked.contains(&claims.login)).then_some(claims)
    }

    /// Attempt a login with anti-replay (amendment §D). The TOTP `code` must match
    /// a step (±1) that is STRICTLY newer than `last_step`, AND — when a password
    /// is enrolled — `password` must match. `kind` is the session length the
    /// operator chose ("keep me signed in" → `Remembered`); it changes only the
    /// cookie's lifetime, never the rigor of the check. Returns the outcome; on
    /// success the caller persists `step` (the new last-consumed step) and sends
    /// `cookie` with the `Max-Age` of `kind`.
    pub fn login_checked(
        &self,
        code: &str,
        password: Option<&str>,
        kind: cookie::SessionKind,
        now: u64,
        last_step: Option<u64>,
    ) -> LoginOutcome {
        let Some(step) = self.totp.matched_step(code, now, 1) else {
            return LoginOutcome::BadCredential;
        };
        if let Some(last) = last_step {
            if step <= last {
                return LoginOutcome::Replayed;
            }
        }
        if let Some(expected) = &self.password {
            match password {
                Some(pw) if expected.verify(pw) => {}
                _ => return LoginOutcome::BadCredential,
            }
        }
        let iat = now;
        let login = cookie::LoginId::mint();
        let cookie = cookie::sign(
            &self.token,
            self.epoch.get(),
            kind,
            iat,
            cookie::slide_exp(kind, iat, now),
            login,
        );
        LoginOutcome::Ok {
            cookie,
            kind,
            step,
            login,
        }
    }

    /// Credential-only login (no anti-replay, standard length): a thin wrapper
    /// over [`login_checked`](Self::login_checked) returning just the cookie.
    /// Kept for callers/tests that don't thread the last-step store.
    pub fn login(&self, code: &str, password: Option<&str>, now: u64) -> Option<String> {
        match self.login_checked(code, password, cookie::SessionKind::Standard, now, None) {
            LoginOutcome::Ok { cookie, .. } => Some(cookie),
            LoginOutcome::BadCredential | LoginOutcome::Replayed => None,
        }
    }

    /// For an authorized session cookie, the re-issued cookie value (and its kind,
    /// for the `Max-Age`) when idle-slide moves `exp` at least the kind's
    /// hysteresis forward (amendment §D), else `None`. Preserves `iat`, the
    /// kind and the login ID, so the absolute cap is never extended and the
    /// re-issued cookie still belongs to the same login.
    pub fn slide_cookie(
        &self,
        cookie_header: Option<&str>,
        now: u64,
    ) -> Option<(String, cookie::SessionKind)> {
        let claims = self.live_claims(cookie_header, now)?;
        let new_exp = cookie::slide_exp(claims.kind, claims.iat, now);
        if new_exp.saturating_sub(claims.exp) < claims.kind.slide_min_secs() {
            return None;
        }
        Some((
            cookie::sign(
                &self.token,
                self.epoch.get(),
                claims.kind,
                claims.iat,
                new_exp,
                claims.login,
            ),
            claims.kind,
        ))
    }
}
