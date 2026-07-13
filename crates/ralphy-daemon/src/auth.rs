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

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::{cookie, password, totp};

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
}

impl SessionAuth {
    /// Whether a `Cookie:` header carries a valid, unexpired session cookie
    /// signed by this daemon's token.
    pub fn cookie_valid(&self, cookie_header: Option<&str>, now: u64) -> bool {
        match cookie::from_cookie_header(cookie_header) {
            Some(value) => cookie::verify(&self.token, &value, now),
            None => false,
        }
    }

    /// Attempt a login: the TOTP `code` must verify (±1 step), AND — when a
    /// password is enrolled — the supplied `password` must match. On success
    /// returns a freshly signed cookie value; `None` on any failure (a missing
    /// password when one is required is a failure).
    pub fn login(&self, code: &str, password: Option<&str>, now: u64) -> Option<String> {
        if !self.totp.verify(code, now, 1) {
            return None;
        }
        if let Some(expected) = &self.password {
            match password {
                Some(pw) if expected.verify(pw) => {}
                _ => return None,
            }
        }
        Some(cookie::sign(&self.token, now + cookie::SESSION_TTL_SECS))
    }
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
) -> AuthPolicy {
    match (policy, token, totp) {
        (AuthPolicy::Bearer(_), Some(key), Some(seed)) => {
            AuthPolicy::Session(Arc::new(SessionAuth {
                token: key,
                totp: seed,
                password,
            }))
        }
        (policy, _, _) => policy,
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

    fn session_over(token: &str) -> AuthPolicy {
        AuthPolicy::Session(Arc::new(SessionAuth {
            token: token.to_string(),
            totp: totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
            password: None,
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
        );
        assert!(
            matches!(promoted, AuthPolicy::Session(_)),
            "Bearer + seed → Session"
        );

        let no_seed =
            upgrade_with_session(AuthPolicy::Bearer("t".into()), Some("t".into()), None, None);
        assert!(
            matches!(no_seed, AuthPolicy::Bearer(_)),
            "Bearer + no seed stays Bearer"
        );

        let local =
            upgrade_with_session(AuthPolicy::Localhost, Some("t".into()), Some(seed()), None);
        assert!(
            matches!(local, AuthPolicy::Localhost),
            "Localhost stays Localhost"
        );
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
