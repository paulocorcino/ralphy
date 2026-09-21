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
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use anyhow::Result;

use crate::{cookie, epoch, password, totp};

mod policy;
mod throttle;
mod token;

pub use policy::{
    compute_policy, require_login_enabled_in, require_login_path_in, set_require_login_in,
    upgrade_with_session, AuthPolicy, LoginOutcome,
};
use throttle::LoginThrottle;
pub(crate) use token::set_owner_only;
pub use token::{
    effective_token, ensure_token_at, generate_token, load_token, load_token_from, save_token_to,
    store_dir, strip_token_from_env, token_path, token_path_in, TOKEN_ENV,
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
}

impl SessionAuth {
    /// Whether a `Cookie:` header carries a valid, unexpired session cookie
    /// signed by this daemon's token AT THE CURRENT EPOCH.
    pub fn cookie_valid(&self, cookie_header: Option<&str>, now: u64) -> bool {
        match cookie::from_cookie_header(cookie_header) {
            Some(value) => cookie::verify(&self.token, self.epoch.get(), &value, now),
            None => false,
        }
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
        let cookie = cookie::sign(
            &self.token,
            self.epoch.get(),
            kind,
            iat,
            cookie::slide_exp(kind, iat, now),
        );
        LoginOutcome::Ok { cookie, kind, step }
    }

    /// Credential-only login (no anti-replay, standard length): a thin wrapper
    /// over [`login_checked`](Self::login_checked) returning just the cookie.
    /// Kept for callers/tests that don't thread the last-step store.
    pub fn login(&self, code: &str, password: Option<&str>, now: u64) -> Option<String> {
        match self.login_checked(code, password, cookie::SessionKind::Standard, now, None) {
            LoginOutcome::Ok { cookie, .. } => Some(cookie),
            _ => None,
        }
    }

    /// For an authorized session cookie, the re-issued cookie value (and its kind,
    /// for the `Max-Age`) when idle-slide moves `exp` at least the kind's
    /// hysteresis forward (amendment §D), else `None`. Preserves `iat` and the
    /// kind, so the absolute cap is never extended.
    pub fn slide_cookie(
        &self,
        cookie_header: Option<&str>,
        now: u64,
    ) -> Option<(String, cookie::SessionKind)> {
        let value = cookie::from_cookie_header(cookie_header)?;
        let claims = cookie::verify_claims(&self.token, self.epoch.get(), &value, now)?;
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
fn host_name(host: &str) -> &str {
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
    throttle: Mutex<LoginThrottle>,
    /// Every host spelling that names this daemon: derived from the bind, plus
    /// whatever the operator declared. See [`allowed_host_set`].
    allowed_hosts: Vec<String>,
    /// The operator-declared subset. Kept apart because a declared host may sit
    /// behind a TLS-terminating reverse proxy, so its origin carries a scheme and
    /// port this listener never sees — [`AuthState::same_origin`] relaxes those two
    /// for declared hosts only, never for the derived ones.
    declared_hosts: Vec<String>,
}

impl AuthState {
    /// Boot the auth state for a bind (the composition-root path). Reads the
    /// on-disk seed/password/require-login flag and computes the initial policy;
    /// fails closed exactly where [`for_bind`] does.
    pub fn boot(
        bound_addr: SocketAddr,
        token: Option<String>,
        epoch: epoch::SessionEpoch,
        declared_hosts: &[String],
    ) -> Result<Arc<AuthState>> {
        let last_step_path = totp::last_step_path_in(&store_dir()?);
        let state = AuthState {
            policy: RwLock::new(AuthPolicy::Localhost),
            bind_ip: bound_addr.ip(),
            bound_addr,
            boot_token: token,
            epoch,
            last_step_path,
            throttle: Mutex::new(LoginThrottle::new()),
            allowed_hosts: allowed_host_set(bound_addr.ip(), declared_hosts),
            declared_hosts: declared_host_set(declared_hosts),
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
            throttle: Mutex::new(LoginThrottle::new()),
            allowed_hosts: allowed_host_set(bind_ip, &[]),
            declared_hosts: Vec::new(),
        })
    }

    /// Wrap a fixed, pre-built policy (tests that drive a specific `Session`/
    /// `Bearer` policy through the router). Rebuild is a no-op relative to the
    /// given policy — it is not recomputed from disk.
    pub fn fixed(policy: AuthPolicy, epoch: epoch::SessionEpoch) -> Arc<AuthState> {
        let bind_ip = IpAddr::from([127, 0, 0, 1]);
        Arc::new(AuthState {
            policy: RwLock::new(policy),
            bind_ip,
            bound_addr: default_bound_addr(),
            boot_token: None,
            epoch,
            last_step_path: detached_last_step_path(),
            throttle: Mutex::new(LoginThrottle::new()),
            allowed_hosts: allowed_host_set(bind_ip, &[]),
            declared_hosts: Vec::new(),
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
    /// The accepted set follows the BIND ([`allowed_host_set`]), not a hardcoded
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
    /// and swap it in. Called after any security mutation so the gate takes effect
    /// immediately. The signing key is the on-disk token if present (so a re-mint
    /// wins), else the boot token.
    pub fn rebuild(&self) -> Result<()> {
        let dir = store_dir()?;
        let token = load_token_from(&token_path_in(&dir))?.or_else(|| self.boot_token.clone());
        let seed = totp::load_seed_from(&totp::seed_path_in(&dir))?;
        let pw = password::load_from(&password::password_path_in(&dir))?;
        let require_login = require_login_enabled_in(&dir);
        let next = compute_policy(
            self.bind_ip,
            token,
            seed,
            pw,
            require_login,
            self.epoch.clone(),
        )?;
        *self.policy.write().expect("auth policy lock poisoned") = next;
        Ok(())
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
    std::env::temp_dir().join(format!("ralphy-laststep-{}", ulid::Ulid::new()))
}

/// Constant-time byte equality: length-checked, then XOR-accumulate over the
/// whole slice so the compare time does not vary with how many leading bytes
/// match. Avoids a timing side-channel on the token.
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An auth state for an arbitrary bind and declared-host list, so the
    /// cross-site gate can be tested on the NETWORK-bind path §4 designs — which
    /// `localhost()`/`fixed()` cannot express, both being pinned to loopback.
    fn state_for(bind: IpAddr, port: u16, declared: &[&str]) -> AuthState {
        let declared: Vec<String> = declared.iter().map(|h| (*h).to_string()).collect();
        AuthState {
            policy: RwLock::new(AuthPolicy::Localhost),
            bind_ip: bind,
            bound_addr: SocketAddr::new(bind, port),
            boot_token: None,
            epoch: epoch::SessionEpoch::in_memory_detached(),
            last_step_path: detached_last_step_path(),
            throttle: Mutex::new(LoginThrottle::new()),
            allowed_hosts: allowed_host_set(bind, &declared),
            declared_hosts: declared_host_set(&declared),
        }
    }

    #[test]
    fn same_origin_refuses_cross_site_and_allows_own_origin() {
        let state = AuthState::localhost();
        let port = crate::DEFAULT_PORT;
        // The daemon's own origin, under every loopback spelling a browser uses.
        assert!(state.same_origin(Some(&format!("http://127.0.0.1:{port}"))));
        assert!(state.same_origin(Some(&format!("http://localhost:{port}"))));
        assert!(state.same_origin(Some(&format!("http://[::1]:{port}"))));
        // A page the operator merely visits — the whole point of the check.
        assert!(!state.same_origin(Some("https://evil.example")));
        // Another app on a DIFFERENT loopback port is a different origin.
        assert!(!state.same_origin(Some(&format!("http://127.0.0.1:{}", port + 1))));
        // `file://` and sandboxed iframes send the opaque origin.
        assert!(!state.same_origin(Some("null")));
        // Absent: same-origin navigation or a non-browser client.
        assert!(state.same_origin(None));
    }

    #[test]
    fn host_allowed_refuses_rebinding_names() {
        let state = AuthState::localhost();
        assert!(state.host_allowed(Some("127.0.0.1:7257")));
        assert!(state.host_allowed(Some("127.0.0.1")));
        assert!(state.host_allowed(Some("localhost:7257")));
        assert!(state.host_allowed(Some("LocalHost")));
        assert!(state.host_allowed(Some("[::1]:7257")));
        // A DNS-rebinding name resolves to loopback but is NOT a loopback name.
        assert!(!state.host_allowed(Some("rebind.evil.example:7257")));
        assert!(!state.host_allowed(Some("192.168.1.10:7257")));
        // Absent: HTTP/1.1 forbids it, so this is a non-browser caller.
        assert!(state.host_allowed(None));
    }

    /// §4 makes a non-loopback bind (a Tailscale interface IP) THE remote-access
    /// path: "personal remote access works in this phase via an overlay VPN with
    /// zero extra code", with a browser logging in over it. A gate pinned to
    /// loopback refused the operator's own browser on every route and every WS
    /// upgrade — the workbench was unreachable from another machine, not merely
    /// degraded. So the accepted set follows the bind.
    #[test]
    fn a_network_bind_accepts_the_browser_that_reaches_it() {
        let bind: IpAddr = "100.64.0.1".parse().expect("a valid test address");
        let state = state_for(bind, 7257, &[]);
        assert!(state.same_origin(Some("http://100.64.0.1:7257")));
        assert!(state.host_allowed(Some("100.64.0.1:7257")));
        assert!(state.host_allowed(Some("100.64.0.1")));
        // Bound to ONE address, the daemon does not answer on loopback, so naming
        // loopback is naming someone else. Narrower than the loopback default.
        assert!(!state.same_origin(Some("http://127.0.0.1:7257")));
        assert!(!state.host_allowed(Some("127.0.0.1:7257")));
        // Everything the gate existed to refuse still is refused.
        assert!(!state.same_origin(Some("http://100.64.0.1:7258")));
        assert!(!state.same_origin(Some("https://evil.example")));
        assert!(!state.same_origin(Some("null")));
        assert!(!state.host_allowed(Some("rebind.evil.example")));
    }

    /// Reaching the daemon by NAME is an explicit declaration, because a name is
    /// exactly what DNS rebinding needs: an undeclared name that re-resolves to
    /// the bound IP must still be refused. This mirrors the `server.allowedHosts`
    /// allowlist Vite added for CVE-2025-24010, whose lesson was that a local
    /// listener is reachable from any page the operator visits.
    #[test]
    fn only_a_declared_name_reaches_the_daemon() {
        let bind: IpAddr = "100.64.0.1".parse().expect("a valid test address");
        let state = state_for(bind, 7257, &["desk.tailnet.ts.net"]);
        assert!(state.host_allowed(Some("desk.tailnet.ts.net:7257")));
        assert!(state.host_allowed(Some("DESK.tailnet.ts.net")));
        assert!(state.same_origin(Some("http://desk.tailnet.ts.net:7257")));
        // A declared host may sit behind a TLS-terminating proxy (`tailscale
        // serve`): https, and the port drops to the scheme default. This listener
        // sees neither, so a declared host matches on the host alone.
        assert!(state.same_origin(Some("https://desk.tailnet.ts.net")));
        // The bound address keeps working alongside the name.
        assert!(state.host_allowed(Some("100.64.0.1")));
        // An undeclared name is refused however it resolves.
        assert!(!state.host_allowed(Some("rebind.evil.example")));
        assert!(!state.same_origin(Some("https://rebind.evil.example")));
        // The relaxation is for DECLARED hosts only — a derived spelling stays
        // pinned to this listener's scheme and port.
        assert!(!state.same_origin(Some("https://100.64.0.1")));
        assert!(!state.same_origin(Some("http://100.64.0.1:7258")));
        // Not an origin at all: an authority-with-path is malformed, and a
        // non-http scheme (an extension page) is never this daemon.
        assert!(!state.same_origin(Some("https://desk.tailnet.ts.net/path")));
        assert!(!state.same_origin(Some("chrome-extension://desk.tailnet.ts.net")));
    }

    /// The operator declares the host by pasting what their tunnel CLI printed —
    /// a full URL. Every spelling of the same endpoint must land on one host, or
    /// the declaration silently misses and every request 403s.
    #[test]
    fn a_declared_host_is_normalized_from_whatever_the_operator_pasted() {
        for pasted in [
            "12ad-203-0-113-7.ngrok-free.app",
            "https://12ad-203-0-113-7.ngrok-free.app/",
            "https://12ad-203-0-113-7.ngrok-free.app",
            "http://12ad-203-0-113-7.ngrok-free.app:443/",
            "  12AD-203-0-113-7.Ngrok-Free.App  ",
        ] {
            assert_eq!(
                normalize_declared(pasted),
                "12ad-203-0-113-7.ngrok-free.app",
                "{pasted:?} must normalize to the bare host"
            );
        }
        // The gate must then accept the endpoint, however it was declared.
        let state = state_for(
            "127.0.0.1".parse().expect("a valid test address"),
            7257,
            &["https://12ad-203-0-113-7.ngrok-free.app/"],
        );
        assert!(state.host_allowed(Some("12ad-203-0-113-7.ngrok-free.app")));
        assert!(state.same_origin(Some("https://12ad-203-0-113-7.ngrok-free.app")));
        // A bracketed IPv6 literal survives the same path.
        assert_eq!(normalize_declared("https://[::1]:7257/"), "::1");
    }

    /// A wildcard bind answers on interfaces we refuse to enumerate by probing the
    /// OS: loopback is known, everything else must be declared.
    #[test]
    fn a_wildcard_bind_keeps_loopback_and_takes_the_rest_as_declared() {
        let state = state_for(
            "0.0.0.0".parse().expect("a valid test address"),
            7257,
            &["desk"],
        );
        assert!(state.same_origin(Some("http://127.0.0.1:7257")));
        assert!(state.same_origin(Some("http://localhost:7257")));
        assert!(state.host_allowed(Some("desk")));
        assert!(!state.host_allowed(Some("192.168.1.10")));
    }

    /// A throwaway in-memory epoch for tests (starts at 0; only bumps when a test
    /// asks). Each call gets its own path so a bump never collides.
    pub(super) fn test_epoch() -> epoch::SessionEpoch {
        let path = std::env::temp_dir().join(format!("ralphy-test-epoch-{}", ulid::Ulid::new()));
        epoch::SessionEpoch::in_memory(0, path)
    }

    pub(super) fn session_over(token: &str) -> AuthPolicy {
        AuthPolicy::Session(Arc::new(SessionAuth {
            token: token.to_string(),
            totp: totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
            password: None,
            epoch: test_epoch(),
        }))
    }

    #[test]
    fn session_login_and_cookie_valid() {
        let s = SessionAuth {
            token: "tok".into(),
            totp: totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
            password: None,
            epoch: test_epoch(),
        };
        // T=59 → RFC vector code 287082 mints a cookie that then validates.
        let cookie = s
            .login("287082", None, 59)
            .expect("valid TOTP mints a cookie");
        let header = format!("{}={cookie}", cookie::COOKIE_NAME);
        assert!(
            s.cookie_valid(Some(&header), 60),
            "the minted cookie authorizes"
        );
        assert!(
            s.login("999999", None, 59).is_none(),
            "a wrong code mints nothing"
        );
    }

    #[test]
    fn session_login_requires_password_when_set() {
        let s = SessionAuth {
            token: "tok".into(),
            totp: totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
            password: Some(password::Hash::hash_password("pw")),
            epoch: test_epoch(),
        };
        assert!(
            s.login("287082", Some("pw"), 59).is_some(),
            "TOTP + right pw logs in"
        );
        assert!(
            s.login("287082", Some("bad"), 59).is_none(),
            "wrong pw fails"
        );
        assert!(
            s.login("287082", None, 59).is_none(),
            "a required pw cannot be omitted"
        );
    }

    #[test]
    fn a_bumped_epoch_invalidates_a_live_session_cookie() {
        // The end-to-end of amendment §B at the auth layer: a cookie minted at
        // epoch N stops verifying once the SHARED epoch is bumped.
        let dir = tempfile::tempdir().unwrap();
        let ep = epoch::SessionEpoch::load(dir.path().join("epoch")).unwrap();
        let s = SessionAuth {
            token: "tok".into(),
            totp: totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
            password: None,
            epoch: ep.clone(),
        };
        let cookie = s.login("287082", None, 59).expect("valid login");
        let header = format!("{}={cookie}", cookie::COOKIE_NAME);
        assert!(s.cookie_valid(Some(&header), 60), "fresh cookie authorizes");
        ep.bump().unwrap();
        assert!(
            !s.cookie_valid(Some(&header), 60),
            "a bumped epoch invalidates the live cookie"
        );
    }

    #[test]
    fn login_checked_rejects_a_replayed_step() {
        let s = SessionAuth {
            token: "tok".into(),
            totp: totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
            password: None,
            epoch: test_epoch(),
        };
        // T=59 → step 1 (59/30), RFC code 287082.
        let out = s.login_checked("287082", None, cookie::SessionKind::Standard, 59, None);
        let step = match out {
            LoginOutcome::Ok { step, .. } => step,
            _ => panic!("first login must succeed"),
        };
        assert_eq!(step, 1);
        // Replaying the same code with that step already consumed is rejected.
        assert!(
            matches!(
                s.login_checked(
                    "287082",
                    None,
                    cookie::SessionKind::Standard,
                    59,
                    Some(step)
                ),
                LoginOutcome::Replayed
            ),
            "a consumed step is a replay"
        );
        // An older last-step still lets the newer step through.
        assert!(matches!(
            s.login_checked("287082", None, cookie::SessionKind::Standard, 59, Some(0)),
            LoginOutcome::Ok { .. }
        ));
    }

    #[test]
    fn slide_cookie_reissues_only_past_the_hysteresis() {
        let s = SessionAuth {
            token: "tok".into(),
            totp: totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
            password: None,
            epoch: test_epoch(),
        };
        // Log in at T=59 to mint a real cookie (iat=59).
        let cookie = s.login("287082", None, 59).expect("login");
        let header = format!("{}={cookie}", cookie::COOKIE_NAME);
        // Immediately: no meaningful slide yet → no re-issue.
        assert!(
            s.slide_cookie(Some(&header), 59).is_none(),
            "no re-issue below the hysteresis"
        );
        // After the hysteresis window, exp has room to slide → re-issue.
        let later = 59 + cookie::SLIDE_MIN_SECS;
        let (_, kind) = s
            .slide_cookie(Some(&header), later)
            .expect("re-issue once activity moves exp forward enough");
        assert_eq!(kind, cookie::SessionKind::Standard, "the kind is preserved");
    }

    #[test]
    fn a_remembered_login_mints_a_remembered_cookie_with_its_own_hysteresis() {
        let s = SessionAuth {
            token: "tok".into(),
            totp: totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
            password: None,
            epoch: test_epoch(),
        };
        let out = s.login_checked("287082", None, cookie::SessionKind::Remembered, 59, None);
        let (cookie, kind) = match out {
            LoginOutcome::Ok { cookie, kind, .. } => (cookie, kind),
            _ => panic!("login must succeed"),
        };
        assert_eq!(kind, cookie::SessionKind::Remembered);
        let claims = cookie::verify_claims("tok", 0, &cookie, 60).expect("verifies");
        assert_eq!(claims.kind, cookie::SessionKind::Remembered);
        assert_eq!(
            claims.exp,
            59 + cookie::REMEMBERED_IDLE_TTL_SECS,
            "a fresh remembered cookie expires a week out"
        );
        let header = format!("{}={cookie}", cookie::COOKIE_NAME);
        // The standard 60 s hysteresis is not enough for a remembered cookie…
        assert!(
            s.slide_cookie(Some(&header), 59 + cookie::SLIDE_MIN_SECS)
                .is_none(),
            "no re-issue below the remembered hysteresis"
        );
        // …an hour is.
        let (_, kind) = s
            .slide_cookie(Some(&header), 59 + cookie::REMEMBERED_SLIDE_MIN_SECS)
            .expect("re-issue past the remembered hysteresis");
        assert_eq!(
            kind,
            cookie::SessionKind::Remembered,
            "the kind is preserved"
        );
    }
}
