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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::{cookie, epoch, password, totp};

/// Env override for the access token: when set non-empty it wins over the
/// on-disk token (a spawned daemon can be handed its token this way). Stripped
/// from the process env at boot so no child inherits it (mirrors
/// `RALPHY_EVENTS_TOKEN`, ADR-0019).
pub const TOKEN_ENV: &str = "RALPHY_DAEMON_TOKEN";

/// The global daemon store root: `$RALPHY_DAEMON_DIR` when set (tests point it at
/// a temp dir), else `<home>/.ralphy` — the same root as `daemon.toml`, never a
/// repo-local `.ralphy/`. The one env-reading resolver the token/seed/password
/// paths and the security routes share.
pub fn store_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_DAEMON_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .context("could not resolve a home directory for the daemon store")?;
    Ok(PathBuf::from(home).join(".ralphy"))
}

/// The `daemon-token` path inside `dir`. Path-explicit so the security routes and
/// tests can point it at a temp dir without touching the process-global env.
pub fn token_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon-token")
}

/// The production path of `daemon-token`. Mirrors [`identity::daemon_toml_path`].
pub fn token_path() -> Result<PathBuf> {
    Ok(token_path_in(&store_dir()?))
}

/// Load the token from `path`, or `Ok(None)` when the file does not exist yet
/// (an un-minted token). Trims a trailing newline so an editor-touched file
/// still compares equal.
pub fn load_token_from(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text.trim_end_matches(['\r', '\n']).to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write `token` to `path` owner-only, creating the parent directory.
pub fn save_token_to(token: &str, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, token).with_context(|| format!("writing {}", path.display()))?;
    set_owner_only(path)?;
    Ok(())
}

/// Generate a fresh access token: 32 CSPRNG bytes (256 bits) hex-encoded to 64
/// chars. No `hex` crate — inline `format!`.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("the OS CSPRNG must be available to mint a token");
    let mut hex = String::with_capacity(64);
    for b in bytes {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Mint-once at `path`: return the existing token with `false`, or generate,
/// save, and return a fresh one with `true`. The `bool` is "was newly minted",
/// so `daemon setup` can show it exactly once.
pub fn ensure_token_at(path: &Path) -> Result<(String, bool)> {
    match load_token_from(path)? {
        Some(token) => Ok((token, false)),
        None => {
            let token = generate_token();
            save_token_to(&token, path)?;
            Ok((token, true))
        }
    }
}

/// Load the current access token from its production path.
pub fn load_token() -> Result<Option<String>> {
    load_token_from(&token_path()?)
}

/// The effective token: a non-empty [`TOKEN_ENV`] override wins, else the
/// on-disk token. `None` when neither resolves.
pub fn effective_token() -> Result<Option<String>> {
    if let Some(v) = std::env::var_os(TOKEN_ENV) {
        let v = v.to_string_lossy().into_owned();
        if !v.is_empty() {
            return Ok(Some(v));
        }
    }
    load_token()
}

/// Remove [`TOKEN_ENV`] from the process environment so no spawned child inherits
/// the access token. Called once at boot after the effective token is captured
/// into the [`AuthPolicy`] (mirrors `strip_events_token_from_env`, ADR-0019).
pub fn strip_token_from_env() {
    std::env::remove_var(TOKEN_ENV);
}

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
    /// is enrolled — `password` must match. Returns the outcome; on success the
    /// caller persists `step` (the new last-consumed step) and sends `cookie`.
    pub fn login_checked(
        &self,
        code: &str,
        password: Option<&str>,
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
            iat,
            cookie::slide_exp(iat, now),
        );
        LoginOutcome::Ok { cookie, step }
    }

    /// Credential-only login (no anti-replay): a thin wrapper over
    /// [`login_checked`](Self::login_checked) returning just the cookie. Kept for
    /// callers/tests that don't thread the last-step store.
    pub fn login(&self, code: &str, password: Option<&str>, now: u64) -> Option<String> {
        match self.login_checked(code, password, now, None) {
            LoginOutcome::Ok { cookie, .. } => Some(cookie),
            _ => None,
        }
    }

    /// For an authorized session cookie, the re-issued cookie value when idle-slide
    /// moves `exp` at least [`cookie::SLIDE_MIN_SECS`] forward (amendment §D), else
    /// `None`. Preserves `iat`, so the absolute cap is never extended.
    pub fn slide_cookie(&self, cookie_header: Option<&str>, now: u64) -> Option<String> {
        let value = cookie::from_cookie_header(cookie_header)?;
        let claims = cookie::verify_claims(&self.token, self.epoch.get(), &value, now)?;
        let new_exp = cookie::slide_exp(claims.iat, now);
        if new_exp.saturating_sub(claims.exp) < cookie::SLIDE_MIN_SECS {
            return None;
        }
        Some(cookie::sign(
            &self.token,
            self.epoch.get(),
            claims.iat,
            new_exp,
        ))
    }
}

/// The outcome of a [`SessionAuth::login_checked`] attempt.
pub enum LoginOutcome {
    /// Credentials verified: send `cookie` and persist `step` as the new last
    /// consumed TOTP step.
    Ok { cookie: String, step: u64 },
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

fn bearer_matches(header: Option<&str>, expected: &str) -> bool {
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

/// A simple global login throttle (amendment §D): a single-operator daemon does
/// not need per-IP buckets, only a brake on online brute force of the 6-digit
/// TOTP. After [`LOCKOUT_THRESHOLD`] consecutive failures it locks out for a
/// window that doubles each further failure, capped at [`LOCKOUT_MAX_SECS`].
struct LoginThrottle {
    failures: u32,
    locked_until: Option<Instant>,
}

/// Consecutive failures tolerated before the lockout engages.
const LOCKOUT_THRESHOLD: u32 = 5;
/// The base lockout window (doubles per failure past the threshold).
const LOCKOUT_BASE_SECS: u64 = 5;
/// The lockout ceiling — never brick the operator out permanently.
const LOCKOUT_MAX_SECS: u64 = 300;

impl LoginThrottle {
    fn new() -> LoginThrottle {
        LoginThrottle {
            failures: 0,
            locked_until: None,
        }
    }

    /// `Err(secs)` with the remaining lockout, or `Ok(())` when a try may proceed.
    fn check(&self, now: Instant) -> std::result::Result<(), u64> {
        match self.locked_until {
            Some(until) if until > now => Err((until - now).as_secs().max(1)),
            _ => Ok(()),
        }
    }

    fn record_failure(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= LOCKOUT_THRESHOLD {
            let over = self.failures - LOCKOUT_THRESHOLD;
            let secs = LOCKOUT_BASE_SECS
                .saturating_mul(1u64 << over.min(6))
                .min(LOCKOUT_MAX_SECS);
            self.locked_until = Some(now + Duration::from_secs(secs));
        }
    }

    fn reset(&mut self) {
        self.failures = 0;
        self.locked_until = None;
    }
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

/// Restrict a freshly written token file to the owner only (mode `0o600` on
/// unix; the per-user home ACL on Windows), mirroring `identity::set_owner_only`.
#[cfg(unix)]
pub(crate) fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("setting owner-only permissions on {}", path.display()))
}

#[cfg(not(unix))]
pub(crate) fn set_owner_only(_path: &Path) -> Result<()> {
    Ok(())
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
            "qnq4phf3-7257.brs.devtunnels.ms",
            "https://qnq4phf3-7257.brs.devtunnels.ms/",
            "https://qnq4phf3-7257.brs.devtunnels.ms",
            "http://qnq4phf3-7257.brs.devtunnels.ms:443/",
            "  QNQ4PHF3-7257.BRS.DevTunnels.ms  ",
        ] {
            assert_eq!(
                normalize_declared(pasted),
                "qnq4phf3-7257.brs.devtunnels.ms",
                "{pasted:?} must normalize to the bare host"
            );
        }
        // The gate must then accept the endpoint, however it was declared.
        let state = state_for(
            "127.0.0.1".parse().expect("a valid test address"),
            7257,
            &["https://qnq4phf3-7257.brs.devtunnels.ms/"],
        );
        assert!(state.host_allowed(Some("qnq4phf3-7257.brs.devtunnels.ms")));
        assert!(state.same_origin(Some("https://qnq4phf3-7257.brs.devtunnels.ms")));
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

    #[test]
    fn localhost_authorizes_without_token() {
        assert!(AuthPolicy::Localhost.authorizes(None));
        assert!(AuthPolicy::Localhost.authorizes(Some("Bearer anything")));
    }

    #[test]
    fn bearer_requires_exact_token() {
        let policy = AuthPolicy::Bearer("s3cret".into());
        assert!(policy.authorizes(Some("Bearer s3cret")));
        assert!(!policy.authorizes(Some("Bearer wrong")));
        assert!(!policy.authorizes(None));
        // A bare token without the `Bearer ` scheme prefix is not authorized.
        assert!(!policy.authorizes(Some("s3cret")));
    }

    /// A throwaway in-memory epoch for tests (starts at 0; only bumps when a test
    /// asks). Each call gets its own path so a bump never collides.
    fn test_epoch() -> epoch::SessionEpoch {
        let path = std::env::temp_dir().join(format!("ralphy-test-epoch-{}", ulid::Ulid::new()));
        epoch::SessionEpoch::in_memory(0, path)
    }

    fn session_over(token: &str) -> AuthPolicy {
        AuthPolicy::Session(Arc::new(SessionAuth {
            token: token.to_string(),
            totp: totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
            password: None,
            epoch: test_epoch(),
        }))
    }

    #[test]
    fn session_authorizes_machine_bearer() {
        // The machine path under Session: a correct `Bearer <token>` still
        // authorizes; a wrong one and a bare cookie-less request do not.
        let policy = session_over("tok");
        assert!(policy.authorizes(Some("Bearer tok")));
        assert!(!policy.authorizes(Some("Bearer wrong")));
        assert!(!policy.authorizes(None));
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
    fn upgrade_with_session_only_promotes_bearer_with_seed() {
        let seed = || totp::Seed::from_bytes(b"12345678901234567890".to_vec());
        let promoted = upgrade_with_session(
            AuthPolicy::Bearer("t".into()),
            Some("t".into()),
            Some(seed()),
            None,
            test_epoch(),
        );
        assert!(
            matches!(promoted, AuthPolicy::Session(_)),
            "Bearer + seed → Session"
        );

        let no_seed = upgrade_with_session(
            AuthPolicy::Bearer("t".into()),
            Some("t".into()),
            None,
            None,
            test_epoch(),
        );
        assert!(
            matches!(no_seed, AuthPolicy::Bearer(_)),
            "Bearer + no seed stays Bearer"
        );

        let local = upgrade_with_session(
            AuthPolicy::Localhost,
            Some("t".into()),
            Some(seed()),
            None,
            test_epoch(),
        );
        assert!(
            matches!(local, AuthPolicy::Localhost),
            "Localhost stays Localhost"
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
        let out = s.login_checked("287082", None, 59, None);
        let step = match out {
            LoginOutcome::Ok { step, .. } => step,
            _ => panic!("first login must succeed"),
        };
        assert_eq!(step, 1);
        // Replaying the same code with that step already consumed is rejected.
        assert!(
            matches!(
                s.login_checked("287082", None, 59, Some(step)),
                LoginOutcome::Replayed
            ),
            "a consumed step is a replay"
        );
        // An older last-step still lets the newer step through.
        assert!(matches!(
            s.login_checked("287082", None, 59, Some(0)),
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
        assert!(
            s.slide_cookie(Some(&header), later).is_some(),
            "re-issue once activity moves exp forward enough"
        );
    }

    #[test]
    fn compute_policy_gates_loopback_only_when_opted_in() {
        let seed = || totp::Seed::from_bytes(b"12345678901234567890".to_vec());
        let loop_ip: IpAddr = "127.0.0.1".parse().unwrap();

        // Default loopback: no gate.
        let p = compute_policy(loop_ip, None, None, None, false, test_epoch()).unwrap();
        assert!(
            matches!(p, AuthPolicy::Localhost),
            "default loopback is open"
        );

        // Opted in + seed + token → gated Session even on loopback.
        let p = compute_policy(
            loop_ip,
            Some("k".into()),
            Some(seed()),
            None,
            true,
            test_epoch(),
        )
        .unwrap();
        assert!(
            matches!(p, AuthPolicy::Session(_)),
            "loopback gate engages with require-login + seed + token"
        );

        // Opted in but NO token to sign with → cannot gate, stays open (safe: it
        // is loopback; the enable route mints a token so this is transient).
        let p = compute_policy(loop_ip, None, Some(seed()), None, true, test_epoch()).unwrap();
        assert!(
            matches!(p, AuthPolicy::Localhost),
            "no signing key → cannot gate loopback"
        );
    }

    #[test]
    fn compute_policy_keeps_network_rules() {
        let seed = || totp::Seed::from_bytes(b"12345678901234567890".to_vec());
        let net_ip: IpAddr = "100.64.0.1".parse().unwrap();
        // Network + no token → fail closed (the §4 invariant).
        assert!(compute_policy(net_ip, None, None, None, false, test_epoch()).is_err());
        // Network + token, no seed → Bearer.
        let p = compute_policy(net_ip, Some("t".into()), None, None, false, test_epoch()).unwrap();
        assert!(matches!(p, AuthPolicy::Bearer(_)));
        // Network + token + seed → Session (unchanged §4 derived behavior).
        let p = compute_policy(
            net_ip,
            Some("t".into()),
            Some(seed()),
            None,
            false,
            test_epoch(),
        )
        .unwrap();
        assert!(matches!(p, AuthPolicy::Session(_)));
    }

    #[test]
    fn require_login_flag_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!require_login_enabled_in(dir.path()), "unset by default");
        set_require_login_in(dir.path(), true).unwrap();
        assert!(require_login_enabled_in(dir.path()), "set → on");
        set_require_login_in(dir.path(), false).unwrap();
        assert!(!require_login_enabled_in(dir.path()), "cleared → off");
        // Idempotent clear.
        set_require_login_in(dir.path(), false).unwrap();
    }

    #[test]
    fn login_throttle_locks_out_after_repeated_failures() {
        let mut t = LoginThrottle::new();
        let t0 = Instant::now();
        // Under the threshold: still open.
        for _ in 0..LOCKOUT_THRESHOLD - 1 {
            t.record_failure(t0);
        }
        assert!(t.check(t0).is_ok(), "not yet locked below the threshold");
        // Crossing the threshold locks out.
        t.record_failure(t0);
        assert!(t.check(t0).is_err(), "locked out after the threshold");
        // A success clears the lockout.
        t.reset();
        assert!(t.check(t0).is_ok(), "reset re-opens");
    }

    #[test]
    fn for_bind_loopback_is_localhost() {
        let policy = AuthPolicy::for_bind("127.0.0.1".parse().unwrap(), None).unwrap();
        assert!(matches!(policy, AuthPolicy::Localhost));
    }

    #[test]
    fn for_bind_network_without_token_errors() {
        assert!(AuthPolicy::for_bind("100.64.0.1".parse().unwrap(), None).is_err());
        // An empty token counts as no token — still fails closed.
        assert!(AuthPolicy::for_bind("100.64.0.1".parse().unwrap(), Some(String::new())).is_err());
    }

    #[test]
    fn for_bind_network_with_token_is_bearer() {
        let policy =
            AuthPolicy::for_bind("100.64.0.1".parse().unwrap(), Some("tok".into())).unwrap();
        assert!(matches!(policy, AuthPolicy::Bearer(t) if t == "tok"));
    }

    #[test]
    fn generate_token_is_64_hex_chars() {
        let token = generate_token();
        assert_eq!(token.len(), 64, "256 bits hex-encoded is 64 chars");
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "token must be lowercase hex; got {token}"
        );
        assert_ne!(token, generate_token(), "two mints must differ");
    }

    #[test]
    fn ensure_token_is_mint_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("daemon-token");
        let (first, minted) = ensure_token_at(&path).unwrap();
        assert!(minted, "first call mints");
        let (second, minted_again) = ensure_token_at(&path).unwrap();
        assert!(!minted_again, "second call does not re-mint");
        assert_eq!(first, second, "the same token is returned");
    }

    #[test]
    fn load_token_from_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_token_from(&dir.path().join("absent")).unwrap(), None);
    }

    #[test]
    fn save_then_load_trims_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon-token");
        save_token_to("abc123\n", &path).unwrap();
        assert_eq!(load_token_from(&path).unwrap(), Some("abc123".into()));
    }
}
