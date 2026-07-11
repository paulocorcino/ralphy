//! The signed, stateless session cookie (issue #179, ADR-0032 §4). A browser
//! login mints `ralphy_session=1.<exp>.<hex HMAC-SHA1(token, "1|<exp>")>`; every
//! subsequent request is authorized by recomputing the MAC (constant-time) and
//! checking `exp > now`. No server-side store — the signing key is the daemon
//! access token, so re-minting the token invalidates outstanding cookies
//! (accepted). Not `Secure`: the daemon never does TLS and rides
//! Tailscale/localhost.

use hmac::{Hmac, Mac};
use sha1::Sha1;

use crate::auth;

/// The session cookie name.
pub const COOKIE_NAME: &str = "ralphy_session";

/// Session lifetime: a fixed 12h — "short-lived" balanced against phone usability
/// for a single-operator daemon behind Tailscale.
pub const SESSION_TTL_SECS: u64 = 12 * 3600;

/// The signed message MAC'd under the token: version-tagged so a format bump is
/// unambiguous. Kept in one place so `sign`/`verify` cannot drift apart.
fn mac_hex(token: &str, exp_unix: u64) -> String {
    let mut mac =
        Hmac::<Sha1>::new_from_slice(token.as_bytes()).expect("HMAC accepts a key of any length");
    mac.update(format!("1|{exp_unix}").as_bytes());
    let bytes = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Mint a cookie value binding `exp_unix` to `token`.
pub fn sign(token: &str, exp_unix: u64) -> String {
    format!("1.{exp_unix}.{}", mac_hex(token, exp_unix))
}

/// Whether `value` is a valid cookie for `token` and not yet expired at
/// `now_unix`. Recomputes the MAC and compares constant-time (`auth::ct_eq`);
/// requires the version tag `1` and `exp > now`.
pub fn verify(token: &str, value: &str, now_unix: u64) -> bool {
    let mut parts = value.splitn(3, '.');
    let (Some(ver), Some(exp_str), Some(mac)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if ver != "1" {
        return false;
    }
    let Ok(exp) = exp_str.parse::<u64>() else {
        return false;
    };
    if exp <= now_unix {
        return false;
    }
    auth::ct_eq(mac.as_bytes(), mac_hex(token, exp).as_bytes())
}

/// The full `Set-Cookie` header value for a freshly minted cookie. `HttpOnly`
/// (no JS access) + `SameSite=Strict` (no cross-site send); NOT `Secure`
/// (see module docs).
pub fn set_cookie_value(cookie: &str) -> String {
    format!("{COOKIE_NAME}={cookie}; HttpOnly; SameSite=Strict; Path=/; Max-Age={SESSION_TTL_SECS}")
}

/// Extract the `ralphy_session` value from a `Cookie:` request header, or `None`
/// when absent. Handles a multi-cookie header (`a=1; ralphy_session=…; b=2`).
pub fn from_cookie_header(header: Option<&str>) -> Option<String> {
    let header = header?;
    for pair in header.split(';') {
        let pair = pair.trim();
        if let Some(value) = pair.strip_prefix(&format!("{COOKIE_NAME}=")) {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_verifies() {
        let c = sign("tok", 1000);
        assert!(verify("tok", &c, 900), "a fresh cookie verifies before exp");
    }

    #[test]
    fn expired_cookie_rejected() {
        let c = sign("tok", 1000);
        assert!(!verify("tok", &c, 1000), "exp == now is expired");
        assert!(!verify("tok", &c, 1500), "past exp is expired");
    }

    #[test]
    fn tampered_mac_rejected() {
        let c = sign("tok", 1000);
        let mut bad = c.clone();
        bad.pop();
        bad.push(if c.ends_with('a') { 'b' } else { 'a' });
        assert!(!verify("tok", &bad, 900), "a tampered MAC must not verify");
    }

    #[test]
    fn wrong_token_rejected() {
        let c = sign("tok", 1000);
        assert!(
            !verify("other", &c, 900),
            "a different signing key must not verify"
        );
    }

    #[test]
    fn malformed_value_rejected() {
        assert!(!verify("tok", "garbage", 900));
        assert!(!verify("tok", "2.1000.deadbeef", 900), "wrong version tag");
        assert!(!verify("tok", "1.notanumber.deadbeef", 900));
    }

    #[test]
    fn set_cookie_has_hardening_attrs() {
        let h = set_cookie_value("1.1000.abc");
        assert!(h.starts_with("ralphy_session=1.1000.abc"));
        assert!(h.contains("HttpOnly") && h.contains("SameSite=Strict") && h.contains("Path=/"));
        assert!(
            !h.contains("Secure"),
            "the cookie is deliberately not Secure"
        );
    }

    #[test]
    fn from_cookie_header_extracts_value() {
        assert_eq!(
            from_cookie_header(Some("a=1; ralphy_session=xyz; b=2")),
            Some("xyz".to_string())
        );
        assert_eq!(
            from_cookie_header(Some("ralphy_session=lone")),
            Some("lone".into())
        );
        assert_eq!(from_cookie_header(Some("other=1")), None);
        assert_eq!(from_cookie_header(None), None);
    }
}
