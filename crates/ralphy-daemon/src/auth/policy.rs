//! The [`AuthPolicy`] (docs/adr/0032 §4 and amendment §A): what a request
//! must carry for the daemon's bind, and how the policy is computed and
//! upgraded from the store.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use super::{ct_eq, set_owner_only, SessionAuth};
use crate::{cookie, epoch, password, totp};

/// The outcome of a [`SessionAuth::login_checked`] attempt.
pub enum LoginOutcome {
    /// Credentials verified: send `cookie` (with the `Max-Age` of `kind`) and
    /// persist `step` as the new last consumed TOTP step.
    Ok {
        cookie: String,
        kind: cookie::SessionKind,
        step: u64,
    },
    /// The code/password did not verify.
    BadCredential,
    /// The code verified but its step was already consumed — a replay.
    Replayed,
}

/// How a request is authorized for the daemon's bind. A loopback bind trusts the
/// local user (no token); a bearer-only network bind requires the exact token; a
/// [`Session`](AuthPolicy::Session) bind additionally accepts a browser session
/// cookie (people) while the bearer still authorizes machines.
#[derive(Clone)]
pub enum AuthPolicy {
    /// Loopback bind: every request is authorized without a token.
    Localhost,
    /// Network bind: only a request carrying `Authorization: Bearer <token>`
    /// with this exact token is authorized.
    Bearer(String),
    /// Hardened network bind: a machine `Bearer <token>` OR a valid browser
    /// session cookie authorizes. The middleware also drives the login flow
    /// (ADR-0032 §4). Additive so the `Localhost`/`Bearer` call sites stay
    /// untouched.
    Session(Arc<SessionAuth>),
}

impl AuthPolicy {
    /// Choose the policy for a bind IP. Loopback → [`AuthPolicy::Localhost`].
    /// Otherwise a non-empty `token` → [`AuthPolicy::Bearer`]; a missing/empty
    /// token FAILS CLOSED with an error naming `ralphy daemon setup` — the daemon
    /// must never begin serving an unauthenticated network socket.
    pub fn for_bind(ip: IpAddr, token: Option<String>) -> Result<AuthPolicy> {
        if ip.is_loopback() {
            return Ok(AuthPolicy::Localhost);
        }
        match token.filter(|t| !t.is_empty()) {
            Some(token) => Ok(AuthPolicy::Bearer(token)),
            None => anyhow::bail!(
                "a non-localhost bind ({ip}) requires an access token, but none is set — \
                 run `ralphy daemon setup` to mint one, or bind 127.0.0.1"
            ),
        }
    }

    /// The wire name of this policy, as reported on `GET /api/session` so the
    /// UI can render honest, bind-specific auth affordances.
    pub fn name(&self) -> &'static str {
        match self {
            AuthPolicy::Localhost => "localhost",
            AuthPolicy::Bearer(_) => "bearer",
            AuthPolicy::Session(_) => "session",
        }
    }

    /// Whether the `Authorization` header authorizes this request. Localhost
    /// always passes; Bearer requires `Bearer <token>` matching the exact token
    /// via a constant-time compare (no timing side-channel on the secret).
    pub fn authorizes(&self, header: Option<&str>) -> bool {
        match self {
            AuthPolicy::Localhost => true,
            AuthPolicy::Bearer(expected) => bearer_matches(header, expected),
            // The machine path under a Session policy: a `Bearer <token>` header
            // still authorizes non-browser clients unchanged (the cookie path is
            // handled by the middleware, which owns `now`).
            AuthPolicy::Session(s) => bearer_matches(header, &s.token),
        }
    }
}

pub(super) fn bearer_matches(header: Option<&str>, expected: &str) -> bool {
    match header.and_then(|h| h.strip_prefix("Bearer ")) {
        Some(got) => ct_eq(got.as_bytes(), expected.as_bytes()),
        None => false,
    }
}

/// Upgrade a resolved bind policy to a browser-session policy when a TOTP seed is
/// enrolled (issue #179). Maps `Bearer(token)` + `Some(seed)` →
/// `Session(SessionAuth{token, seed, password})`; leaves `Localhost`, and a
/// `Bearer` with no seed, unchanged — honoring the opt-in posture (a network
/// bind with no seed stays bearer-only). `token` is the effective access token
/// captured BEFORE it is stripped from the env; it becomes the cookie signing
/// key.
pub fn upgrade_with_session(
    policy: AuthPolicy,
    token: Option<String>,
    totp: Option<totp::Seed>,
    password: Option<password::Hash>,
    epoch: epoch::SessionEpoch,
) -> AuthPolicy {
    match (policy, token, totp) {
        (AuthPolicy::Bearer(_), Some(key), Some(seed)) => {
            AuthPolicy::Session(Arc::new(SessionAuth {
                token: key,
                totp: seed,
                password,
                epoch,
            }))
        }
        (policy, _, _) => policy,
    }
}

/// The `daemon-require-login` flag path inside `dir`. Its PRESENCE means the
/// operator opted the browser UI behind a login gate — including on a loopback
/// bind (ADR-0032 amendment §A). Path-explicit like the token/seed stores.
pub fn require_login_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon-require-login")
}

/// Whether the require-login flag is set under `dir` (the file exists).
pub fn require_login_enabled_in(dir: &Path) -> bool {
    require_login_path_in(dir).exists()
}

/// Set or clear the require-login flag under `dir`. Enabling writes the marker
/// owner-only; disabling removes it (idempotent).
pub fn set_require_login_in(dir: &Path, enable: bool) -> Result<()> {
    let path = require_login_path_in(dir);
    if enable {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, "1").with_context(|| format!("writing {}", path.display()))?;
        set_owner_only(&path)?;
        Ok(())
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
        }
    }
}

/// Compute the effective policy from the resolved inputs (ADR-0032 §4 +
/// amendment §A). A loopback bind is `Localhost` UNLESS the operator opted into
/// require-login AND a TOTP seed is armed AND a signing token exists, in which
/// case it is gated (`Session`). A network bind keeps the §4 rule: `Bearer`, or
/// `Session` once a seed is armed. Fails closed exactly where [`for_bind`] does
/// (a network bind with no token).
pub fn compute_policy(
    bind_ip: IpAddr,
    token: Option<String>,
    seed: Option<totp::Seed>,
    password: Option<password::Hash>,
    require_login: bool,
    epoch: epoch::SessionEpoch,
) -> Result<AuthPolicy> {
    match AuthPolicy::for_bind(bind_ip, token.clone())? {
        AuthPolicy::Localhost => match (require_login, token, seed) {
            (true, Some(key), Some(seed)) => Ok(AuthPolicy::Session(Arc::new(SessionAuth {
                token: key,
                totp: seed,
                password,
                epoch,
            }))),
            // Gate requested but no seed/token to enforce it → stay open (the
            // enable route mints a token and refuses without a seed, so this is
            // only the transient/invalid case). Fail OPEN here is safe: it is a
            // loopback bind, the §4 default.
            _ => Ok(AuthPolicy::Localhost),
        },
        bearer @ AuthPolicy::Bearer(_) => {
            Ok(upgrade_with_session(bearer, token, seed, password, epoch))
        }
        session => Ok(session),
    }
}

#[cfg(test)]
mod tests;
