//! The daemon's bind policy and access token (docs/adr/0032 §4): the guardrail
//! that keeps a network `--bind` from ever serving an unauthenticated socket.
//!
//! Two pieces live here:
//! - The **token store**: a mint-once 256-bit access token in the global store
//!   (`<home>/.ralphy/daemon-token`, mode 0600), a SEPARATE file from
//!   `daemon.toml` so its lifecycle survives a re-`daemon setup` (which
//!   overwrites name/avatar via `identity::baptize`).
//! - The **[`AuthPolicy`]**: a loopback bind serves without a token; a
//!   non-loopback bind REQUIRES a bearer and fails closed when none resolves —
//!   the daemon must never begin serving an unauthenticated network listener.
//!
//! Pure sync, path-explicit like `identity`: tests pass a temp path and never
//! mutate the process-global env (the `RALPHY_*_DIR` env-race trap).

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use anyhow::Result;
use subtle::ConstantTimeEq;

use crate::{cookie, epoch, password, totp};

mod host;
mod policy;
mod revoked;
mod session;
mod throttle;
mod token;

pub(crate) use host::host_name;
#[cfg(test)]
use host::normalize_declared;
use host::{allowed_host_set, declared_host_set, host_eq, origin_host};
pub use policy::{
    compute_policy, remote_images_enabled_in, remote_images_path_in, require_login_enabled_in,
    require_login_path_in, require_token_enabled_in, require_token_path_in, set_remote_images_in,
    set_require_login_in, set_require_token_in, upgrade_with_session, AuthPolicy, LoginOutcome,
    Requirements,
};
pub use revoked::{revoked_logins_path_in, RevokedLogins};
pub use session::SessionAuth;
use throttle::LoginThrottle;
pub use token::{
    effective_token, effective_token_from, ensure_token_at, env_token, generate_token, load_token,
    load_token_from, os_default_store, same_dir, save_token_to, store_dir, token_path,
    token_path_in, TOKEN_ENV,
};

/// The bound address the credential-free constructors report. Tests build a
/// router without a listener, so there is no real address to name; the default
/// port keeps [`AuthState::same_origin`] honest for anything that does send an
/// `Origin`.
fn default_bound_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], crate::DEFAULT_PORT))
}

/// The daemon's live auth state (ADR-0032 amendment §A/§B): a runtime-swappable
/// [`AuthPolicy`] plus the identity needed to recompute it (bind IP, signing
/// token, session epoch). The middleware and the login/security routes hold an
/// `Arc<AuthState>`; a security mutation calls [`AuthState::rebuild`] and the
/// next request sees the new policy — no restart. This is the state §4 captured
/// once at boot, now made mutable behind an `RwLock` (cheap: the policy clones
/// via `Arc`, and the guard is never held across an `.await`).
pub struct AuthState {
    policy: RwLock<AuthPolicy>,
    bind_ip: IpAddr,
    /// The address the listener actually bound, so the middleware can name the
    /// daemon's OWN origin when refusing a cross-site request. The port matters:
    /// another app on a different loopback port is a different origin.
    bound_addr: SocketAddr,
    /// The token captured at boot — the signing-key fallback when the on-disk
    /// token is absent (e.g. handed via env then stripped). A re-mint writes disk,
    /// which then wins in [`AuthState::rebuild`].
    boot_token: Option<String>,
    epoch: epoch::SessionEpoch,
    /// The anti-replay last-step store path (amendment §D). The real store under
    /// `boot`; a detached temp path for `localhost`/`fixed` so tests never touch
    /// the global store.
    last_step_path: PathBuf,
    /// The revoked-login list, shared with any `Session` policy inside.
    revoked: RevokedLogins,
    /// The file of the list: the real store under `boot`, a detached temp
    /// path for `localhost`/`fixed`, as `last_step_path` is.
    revoked_path: PathBuf,
    /// Held across the read, change and write of a revoke, so two revokes at
    /// once do not lose one.
    revoke_lock: Mutex<()>,
    throttle: Mutex<LoginThrottle>,
    /// Every host spelling that names this daemon: derived from the bind, plus
    /// whatever the operator declared. See [`allowed_host_set`].
    allowed_hosts: Vec<String>,
    /// The operator-declared subset. Kept apart because a declared host may sit
    /// behind a TLS-terminating reverse proxy, so its origin carries a scheme and
    /// port this listener never sees — [`AuthState::same_origin`] relaxes those two
    /// for declared hosts only, never for the derived ones.
    declared_hosts: Vec<String>,
    /// The remote-images flag as last read from disk by [`AuthState::rebuild`],
    /// so the response-header layer picks its CSP without a disk read per
    /// response. Independent data: `Relaxed` is enough.
    remote_images: AtomicBool,
}

impl AuthState {
    /// Boot the auth state for a bind (the composition-root path). Reads the
    /// on-disk seed/password/require-login flag and computes the initial policy;
    /// fails closed exactly where `for_bind` does.
    pub fn boot(
        bound_addr: SocketAddr,
        token: Option<String>,
        epoch: epoch::SessionEpoch,
        declared_hosts: &[String],
    ) -> Result<Arc<AuthState>> {
        let dir = store_dir()?;
        let state = AuthState {
            policy: RwLock::new(AuthPolicy::Localhost),
            bind_ip: bound_addr.ip(),
            bound_addr,
            boot_token: token,
            epoch,
            last_step_path: totp::last_step_path_in(&dir),
            revoked: RevokedLogins::default(),
            revoked_path: revoked_logins_path_in(&dir),
            revoke_lock: Mutex::new(()),
            throttle: Mutex::new(LoginThrottle::new()),
            allowed_hosts: allowed_host_set(bound_addr.ip(), declared_hosts),
            declared_hosts: declared_host_set(declared_hosts),
            remote_images: AtomicBool::new(false),
        };
        state.rebuild()?;
        Ok(Arc::new(state))
    }

    /// A localhost auth state for tests and callers that want the frictionless
    /// default (no token, no gate). Never fails.
    pub fn localhost() -> Arc<AuthState> {
        let bind_ip = IpAddr::from([127, 0, 0, 1]);
        Arc::new(AuthState {
            policy: RwLock::new(AuthPolicy::Localhost),
            bind_ip,
            bound_addr: default_bound_addr(),
            boot_token: None,
            epoch: epoch::SessionEpoch::in_memory_detached(),
            last_step_path: detached_last_step_path(),
            revoked: RevokedLogins::default(),
            revoked_path: detached_revoked_path(),
            revoke_lock: Mutex::new(()),
            throttle: Mutex::new(LoginThrottle::new()),
            allowed_hosts: allowed_host_set(bind_ip, &[]),
            declared_hosts: Vec::new(),
            remote_images: AtomicBool::new(false),
        })
    }

    /// Wrap a fixed, pre-built policy (tests that drive a specific `Session`/
    /// `Bearer` policy through the router). Rebuild is a no-op relative to the
    /// given policy — it is not recomputed from disk.
    /// A `Session` policy keeps its own revoked-login list: a revoke on this
    /// state changes that list.
    pub fn fixed(policy: AuthPolicy, epoch: epoch::SessionEpoch) -> Arc<AuthState> {
        let bind_ip = IpAddr::from([127, 0, 0, 1]);
        let revoked = match &policy {
            AuthPolicy::Session(s) => s.revoked.clone(),
            AuthPolicy::Localhost | AuthPolicy::Bearer(_) => RevokedLogins::default(),
        };
        Arc::new(AuthState {
            policy: RwLock::new(policy),
            bind_ip,
            bound_addr: default_bound_addr(),
            boot_token: None,
            epoch,
            last_step_path: detached_last_step_path(),
            revoked,
            revoked_path: detached_revoked_path(),
            revoke_lock: Mutex::new(()),
            throttle: Mutex::new(LoginThrottle::new()),
            allowed_hosts: allowed_host_set(bind_ip, &[]),
            declared_hosts: Vec::new(),
            remote_images: AtomicBool::new(false),
        })
    }

    /// The port this daemon's listener actually bound. The peer probe compares a
    /// descriptor's announced port against it to refuse dialling itself
    /// (ADR-0052 §2 — both daemons default to the same port and WSL's relay
    /// publishes the peer's on this side).
    pub fn bound_port(&self) -> u16 {
        self.bound_addr.port()
    }

    /// Whether this request's `Origin` is the daemon's own, i.e. NOT cross-site.
    ///
    /// Loopback is not a trust boundary against the operator's OWN browser: any
    /// web page they visit can address `127.0.0.1`, and a WebSocket handshake is
    /// never CORS-preflighted — RFC 6455 §10.2 puts the origin check on the
    /// server. A browser always supplies `Origin` on scripted WS connections and
    /// on cross-site form POSTs, so a present-and-foreign `Origin` is the signal.
    ///
    /// An ABSENT `Origin` passes: that is a same-origin navigation or a
    /// non-browser client (curl, a machine bearer client), which loopback already
    /// treats as the operator. A browser cannot suppress the header on the paths
    /// that matter, so this is not a browser-reachable hole.
    ///
    /// The accepted set follows the BIND (`allowed_host_set`), not a hardcoded
    /// loopback list: §4 makes a network bind the remote-access path, and pinning
    /// this to loopback refused the operator's own browser over Tailscale.
    pub fn same_origin(&self, origin: Option<&str>) -> bool {
        let Some(origin) = origin else { return true };
        let port = self.bound_addr.port();
        // `null` (sandboxed iframe, `file://` document) has no `://` and matches
        // nothing below — it stays explicitly foreign.
        if self.allowed_hosts.iter().any(|host| {
            origin.eq_ignore_ascii_case(&format!("http://{}:{port}", origin_host(host)))
        }) {
            return true;
        }
        // A DECLARED host is the operator's own front door and may be fronted by a
        // TLS-terminating reverse proxy (`tailscale serve`), which rewrites the
        // scheme to https and drops the port to the scheme default — neither of
        // which this listener can know. So declared hosts match on the host alone.
        // Derived spellings stay strict above: another app on a different port of
        // the same address is a different origin.
        let Some((scheme, authority)) = origin.split_once("://") else {
            return false;
        };
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return false;
        }
        // An origin is scheme + authority only; anything with a path is malformed.
        if authority.is_empty() || authority.contains('/') {
            return false;
        }
        let name = host_name(authority);
        !name.is_empty() && self.declared_hosts.iter().any(|h| host_eq(h, name))
    }

    /// Whether this request's `Host` names an address this daemon answers on.
    /// Defence against DNS rebinding: an attacker domain that re-resolves to the
    /// daemon's IP becomes genuinely same-origin, so `Origin` alone would accept
    /// its later requests. A NAME is what rebinding needs, and a name passes only
    /// if the operator declared it. An absent `Host` passes — HTTP/1.1 requires
    /// one, so absence means a non-browser caller (and axum's own test transport
    /// omits it).
    pub fn host_allowed(&self, host: Option<&str>) -> bool {
        let Some(host) = host else { return true };
        let name = host_name(host);
        if name.is_empty() {
            return false;
        }
        // Under a loopback or wildcard bind, any loopback literal is the daemon
        // itself (Linux routes all of 127/8 there). Literals are not rebindable.
        if (self.bind_ip.is_loopback() || self.bind_ip.is_unspecified())
            && name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
        {
            return true;
        }
        self.allowed_hosts.iter().any(|h| host_eq(h, name))
    }

    /// The last consumed TOTP step (anti-replay, amendment §D), or `None`.
    pub fn last_step(&self) -> Option<u64> {
        totp::load_last_step_from(&self.last_step_path)
            .ok()
            .flatten()
    }

    /// Record a consumed TOTP step. Best-effort: a failed write only weakens
    /// anti-replay, never blocks a valid login.
    pub fn record_step(&self, step: u64) {
        if let Err(e) = totp::save_last_step_to(step, &self.last_step_path) {
            tracing::warn!(error = %e, "failed to record the TOTP step for anti-replay");
        }
    }

    /// The current policy (cheap clone under a short read lock).
    pub fn policy(&self) -> AuthPolicy {
        self.policy
            .read()
            .expect("auth policy lock poisoned")
            .clone()
    }

    /// The live session epoch (shared with any `Session` policy inside).
    pub fn epoch(&self) -> &epoch::SessionEpoch {
        &self.epoch
    }

    /// Recompute the policy from disk (seed, password, require-login flag, token)
    /// and swap it in, and re-read the remote-images flag and the revoked-login
    /// list. Called after any security mutation so the gate takes effect
    /// immediately. The signing key is the on-disk token if present (so a
    /// re-mint wins), else the boot token.
    pub fn rebuild(&self) -> Result<()> {
        let dir = store_dir()?;
        let token = load_token_from(&token_path_in(&dir))?.or_else(|| self.boot_token.clone());
        let seed = totp::load_seed_from(&totp::seed_path_in(&dir))?;
        let pw = password::load_from(&password::password_path_in(&dir))?;
        let require_login = require_login_enabled_in(&dir);
        self.reload_revoked()?;
        let next = policy::compute_policy_with(
            self.bind_ip,
            token,
            seed,
            pw,
            Requirements {
                login: require_login,
                token: require_token_enabled_in(&dir)?,
            },
            policy::Shared {
                epoch: self.epoch.clone(),
                revoked: self.revoked.clone(),
            },
        )?;
        *self.policy.write().expect("auth policy lock poisoned") = next;
        self.remote_images
            .store(remote_images_enabled_in(&dir), Ordering::Relaxed);
        Ok(())
    }

    /// Read the revoked-login list from disk into the shared handle. It holds
    /// `revoke_lock` from the read to the swap, so a revoke cannot land
    /// between them and be undone in memory.
    fn reload_revoked(&self) -> Result<()> {
        let _held = self.revoke_lock.lock().expect("revoke lock poisoned");
        let entries = revoked::load(&self.revoked_path, crate::routes::now_unix())?;
        self.revoked.replace(entries);
        Ok(())
    }

    /// Whether the operator opted into remote images (the CSP `img-src https:`).
    pub fn remote_images(&self) -> bool {
        self.remote_images.load(Ordering::Relaxed)
    }

    /// Revoke one login: put `login` on the revoked-login list until no cookie
    /// of it can still be valid, write the list, and refuse that login's
    /// cookies from the next check on. The list is not changed when the write
    /// fails.
    pub fn revoke_login(&self, login: cookie::LoginId, now: u64) -> Result<()> {
        let _held = self.revoke_lock.lock().expect("revoke lock poisoned");
        let entries = revoked::load(&self.revoked_path, now)?;
        let cap = now.saturating_add(cookie::REMEMBERED_ABSOLUTE_TTL_SECS);
        let entries = revoked::add(entries, login, cap, now);
        revoked::write(&self.revoked_path, &entries)?;
        self.revoked.replace(entries);
        Ok(())
    }

    /// Whether `login` was revoked.
    pub fn login_revoked(&self, login: &cookie::LoginId) -> bool {
        self.revoked.contains(login)
    }

    /// Invalidate every outstanding session cookie (bump the epoch). Real
    /// server-side logout (amendment §B).
    pub fn invalidate_sessions(&self) -> Result<()> {
        self.epoch.bump().map(|_| ())
    }

    /// Consult the login throttle: `Err(retry_after_secs)` while locked out,
    /// `Ok(())` when a login attempt may proceed (amendment §D).
    pub fn throttle_check(&self) -> std::result::Result<(), u64> {
        self.throttle
            .lock()
            .expect("throttle lock poisoned")
            .check(Instant::now())
    }

    /// Record a failed login (grows the lockout) or a success (clears it).
    pub fn throttle_record(&self, success: bool) {
        let mut t = self.throttle.lock().expect("throttle lock poisoned");
        if success {
            t.reset();
        } else {
            t.record_failure(Instant::now());
        }
    }
}

/// A unique throwaway path for a detached auth state's anti-replay store, so
/// tests and the frictionless `Localhost`/`fixed` states never write the real
/// global store (mirrors [`epoch::SessionEpoch::in_memory_detached`]).
fn detached_last_step_path() -> PathBuf {
    detached_store_file("ralphy-laststep", "daemon-totp-laststep")
}

/// A unique throwaway path for a detached auth state's revoked-login list.
fn detached_revoked_path() -> PathBuf {
    detached_store_file("ralphy-revoked", "daemon-revoked-logins")
}

/// `file` in a new directory `<prefix>-<ulid>` of the temp dir.
fn detached_store_file(prefix: &str, file: &str) -> PathBuf {
    // In its own directory: a write makes the parent owner-only, and on Linux
    // the shared temp dir belongs to root, so a chmod there fails and the file
    // is never written.
    std::env::temp_dir()
        .join(format!("{prefix}-{}", ulid::Ulid::new()))
        .join(file)
}

/// Constant-time byte equality: length-checked, then `subtle` compares the
/// whole slice, so the compare time does not vary with how many leading bytes
/// match. Avoids a timing side-channel on the token. The length is not secret.
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

#[cfg(test)]
mod tests;
