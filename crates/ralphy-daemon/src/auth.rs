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

mod policy;
mod revoked;
mod throttle;
mod token;

pub use policy::{
    compute_policy, remote_images_enabled_in, remote_images_path_in, require_login_enabled_in,
    require_login_path_in, require_token_enabled_in, require_token_path_in, set_remote_images_in,
    set_require_login_in, set_require_token_in, upgrade_with_session, AuthPolicy, LoginOutcome,
    Requirements,
};
pub use revoked::{revoked_logins_path_in, RevokedLogins};
use throttle::LoginThrottle;
pub use token::{
    effective_token, effective_token_from, ensure_token_at, env_token, generate_token, load_token,
    load_token_from, save_token_to, store_dir, token_path, token_path_in, TOKEN_ENV,
};

/// The browser-session credentials for a hardened network bind (issue #179): the
/// signing-key token, the enrolled TOTP seed, and an OPTIONAL password. Held
/// behind an `Arc` in [`AuthPolicy::Session`] so the policy stays cheap to clone.
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
    /// The revoked-login list, shared with the [`AuthState`] that revokes: a
    /// cookie of a login on it is refused.
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

/// Whether an `Authorization` header is `Bearer <token>` matching `expected`
/// (constant-time). Shared by the `Bearer` and `Session` (machine) arms.
/// The bound address the credential-free constructors report. Tests build a
/// router without a listener, so there is no real address to name; the default
/// port keeps [`AuthState::same_origin`] honest for anything that does send an
/// `Origin`.
fn default_bound_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], crate::DEFAULT_PORT))
}

/// The host part of a `Host` header, minus any port. Handles the bracketed IPv6
/// form (`[::1]:7257`), where a plain `rsplit_once(':')` would cut the address.
pub(crate) fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

/// The host spellings that name the daemon ITSELF for a given bind — the names a
/// browser can legitimately have in its address bar, and therefore the only ones
/// allowed in `Origin`/`Host`.
///
/// Derived from the bind, not hardcoded: §4 designs a non-loopback bind (a
/// Tailscale interface IP) as the remote-access path, so pinning this to loopback
/// refuses the operator's own browser on every route. A specific non-loopback
/// bind makes loopback UNREACHABLE, so it is dropped from the set — naming an
/// address the daemon does not answer on would only widen the allowlist.
///
/// A wildcard bind (`0.0.0.0`/`::`) answers on interfaces we cannot enumerate
/// without probing the OS; loopback is included and everything else must be
/// declared. That is the `configured` list: an operator-supplied allowlist of
/// names they reach the daemon by (MagicDNS, a reverse-proxy hostname). It is
/// also the DNS-rebinding boundary — an attacker domain that re-resolves to the
/// daemon's IP is refused unless the operator named it.
fn allowed_host_set(bound_ip: IpAddr, configured: &[String]) -> Vec<String> {
    let mut hosts = Vec::new();
    if bound_ip.is_loopback() || bound_ip.is_unspecified() {
        hosts.push("127.0.0.1".to_string());
        hosts.push("localhost".to_string());
        hosts.push("::1".to_string());
    }
    if !bound_ip.is_loopback() && !bound_ip.is_unspecified() {
        hosts.push(bound_ip.to_string());
    }
    hosts.extend(declared_host_set(configured));
    hosts
}

/// The operator-declared hosts, normalized, with blanks dropped.
fn declared_host_set(configured: &[String]) -> Vec<String> {
    configured
        .iter()
        .map(|h| normalize_declared(h))
        .filter(|h| !h.is_empty())
        .collect()
}

/// Normalize one declared host: drop the scheme, the path, and the port, then
/// lowercase.
///
/// Accepting a whole URL is not politeness, it is the failure mode. Tunnel CLIs
/// print the endpoint as `https://<host>/`, so pasting that is the obvious
/// gesture — and a bare `host_name` would take everything before the LAST colon
/// and silently declare the host `https`. The operator then gets a blanket 403
/// with nothing naming the cause.
fn normalize_declared(raw: &str) -> String {
    let host = raw.trim();
    let host = host.split_once("://").map_or(host, |(_, rest)| rest);
    let host = host.split('/').next().unwrap_or(host);
    host_name(host).to_ascii_lowercase()
}

/// A host spelled for the authority part of an origin: IPv6 needs brackets.
fn origin_host(host: &str) -> String {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(ip)) => format!("[{ip}]"),
        _ => host.to_string(),
    }
}

/// Whether two host strings name the same host. IPs compare as parsed addresses
/// so alternate spellings of one address agree; names compare ASCII-caselessly.
fn host_eq(a: &str, b: &str) -> bool {
    match (a.parse::<IpAddr>(), b.parse::<IpAddr>()) {
        (Ok(a), Ok(b)) => a == b,
        (Err(_), Err(_)) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
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
    /// The port this daemon's listener actually bound. The peer probe compares a
    /// descriptor's announced port against it to refuse dialling itself
    /// (ADR-0052 §2 — both daemons default to the same port and WSL's relay
    /// publishes the peer's on this side).
    pub fn bound_port(&self) -> u16 {
        self.bound_addr.port()
    }

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
