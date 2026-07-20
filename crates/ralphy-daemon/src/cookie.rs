//! The signed, stateless session cookie (issue #179, ADR-0032 §4 + amendment §B).
//! A browser login mints
//! `ralphy_session=2.<epoch>.<exp>.<hex HMAC-SHA1(token, "2|<epoch>|<exp>")>`;
//! every subsequent request is authorized by recomputing the MAC (constant-time)
//! and checking `exp > now`. No server-side store — the signing key is the daemon
//! access token (re-minting it invalidates outstanding cookies), and the
//! **session epoch** binds each cookie to the current epoch so a bump (logout,
//! re-mint, TOTP revoke, disabling require-login) invalidates every live cookie
//! instantly (amendment §B). Not `Secure`: the daemon never does TLS and rides
//! Tailscale/localhost.
//!
//! Format history: `1.<exp>.<mac>` (pre-epoch) no longer verifies — a `1.*`
//! cookie fails closed and the browser re-logs in once.

use hmac::{Hmac, Mac};
use sha1::Sha1;

use crate::auth;

/// The session cookie name.
pub const COOKIE_NAME: &str = "ralphy_session";

/// The ABSOLUTE session cap: 12h from login, never extended (amendment §D). A
/// hard ceiling even under continuous activity.
pub const SESSION_ABSOLUTE_TTL_SECS: u64 = 12 * 3600;

/// The IDLE window: a cookie unused for this long expires (amendment §D). Each
/// authorized request slides `exp` forward to `now + IDLE`, bounded by the
/// absolute cap — so 30 min of inactivity logs the operator out.
pub const SESSION_IDLE_TTL_SECS: u64 = 30 * 60;

/// Re-issue hysteresis: a slid cookie is only re-emitted when it moves `exp` at
/// least this far, so activity produces at most one `Set-Cookie` per minute.
pub const SLIDE_MIN_SECS: u64 = 60;

/// The validated claims carried by a session cookie: when it was issued (`iat`,
/// the anchor for the absolute cap) and when it currently expires (`exp`, slid by
/// activity within the cap).
pub struct Claims {
    pub iat: u64,
    pub exp: u64,
}

/// The slid expiry for a cookie issued at `iat`, seen at `now`: `now + IDLE`
/// clamped to the absolute cap `iat + ABSOLUTE`. Monotonic non-decreasing as
/// `now` advances until it freezes at the cap.
pub fn slide_exp(iat: u64, now: u64) -> u64 {
    (now + SESSION_IDLE_TTL_SECS).min(iat + SESSION_ABSOLUTE_TTL_SECS)
}

/// The signed message MAC'd under the token: version-tagged, epoch-bound, and
/// binding both `iat` and `exp` so neither can be tampered. Kept in one place so
/// `sign`/`verify` cannot drift apart.
fn mac_hex(token: &str, epoch: u64, iat: u64, exp: u64) -> String {
    let mut mac =
        Hmac::<Sha1>::new_from_slice(token.as_bytes()).expect("HMAC accepts a key of any length");
    mac.update(format!("2|{epoch}|{iat}|{exp}").as_bytes());
    let bytes = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Mint a cookie value binding `epoch`, `iat`, and `exp` to `token`.
pub fn sign(token: &str, epoch: u64, iat: u64, exp: u64) -> String {
    format!("2.{epoch}.{iat}.{exp}.{}", mac_hex(token, epoch, iat, exp))
}

/// The validated claims of `value` for `token` at the current `epoch`, or `None`
/// when it is malformed, wrong-epoch, tampered, or expired at `now_unix`.
/// Requires the version tag `2`, the matching epoch, and `exp > now`. A cookie
/// carrying any other epoch fails closed (invalidated by a bump).
pub fn verify_claims(token: &str, epoch: u64, value: &str, now_unix: u64) -> Option<Claims> {
    let mut parts = value.splitn(5, '.');
    let (Some(ver), Some(epoch_str), Some(iat_str), Some(exp_str), Some(mac)) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    if ver != "2" {
        return None;
    }
    let cookie_epoch = epoch_str.parse::<u64>().ok()?;
    if cookie_epoch != epoch {
        return None;
    }
    let iat = iat_str.parse::<u64>().ok()?;
    let exp = exp_str.parse::<u64>().ok()?;
    if exp <= now_unix {
        return None;
    }
    if !auth::ct_eq(mac.as_bytes(), mac_hex(token, epoch, iat, exp).as_bytes()) {
        return None;
    }
    Some(Claims { iat, exp })
}

/// Whether `value` is a valid, unexpired cookie for `token` at `epoch`.
pub fn verify(token: &str, epoch: u64, value: &str, now_unix: u64) -> bool {
    verify_claims(token, epoch, value, now_unix).is_some()
}

/// The full `Set-Cookie` header value for a freshly minted cookie. `HttpOnly`
/// (no JS access) + `SameSite=Strict` (no cross-site send); NOT `Secure`
/// (see module docs). `Max-Age` tracks the idle window so the browser also drops
/// an idle cookie; server-side the absolute cap still bounds it.
pub fn set_cookie_value(cookie: &str) -> String {
    format!(
        "{COOKIE_NAME}={cookie}; HttpOnly; SameSite=Strict; Path=/; Max-Age={SESSION_IDLE_TTL_SECS}"
    )
}

/// The `Set-Cookie` header value that CLEARS the session cookie: an empty value
/// with `Max-Age=0` so the browser drops it immediately. Same attributes as
/// [`set_cookie_value`] so the clear matches the original scope (issue #186).
pub fn clear_cookie_value() -> String {
    format!("{COOKIE_NAME}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")
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
        let c = sign("tok", 0, 500, 1000);
        let claims = verify_claims("tok", 0, &c, 900).expect("a fresh cookie verifies before exp");
        assert_eq!((claims.iat, claims.exp), (500, 1000), "claims round-trip");
    }

    #[test]
    fn expired_cookie_rejected() {
        let c = sign("tok", 0, 500, 1000);
        assert!(!verify("tok", 0, &c, 1000), "exp == now is expired");
        assert!(!verify("tok", 0, &c, 1500), "past exp is expired");
    }

    #[test]
    fn a_bumped_epoch_invalidates_the_cookie() {
        // The core of amendment §B: the same token + exp under a newer epoch no
        // longer verifies — a bump is instant, total logout.
        let c = sign("tok", 3, 500, 1000);
        assert!(verify("tok", 3, &c, 900), "verifies under its own epoch");
        assert!(
            !verify("tok", 4, &c, 900),
            "a bumped epoch invalidates the cookie"
        );
        assert!(
            !verify("tok", 2, &c, 900),
            "an older epoch also rejects (exact match required)"
        );
    }

    #[test]
    fn slide_exp_slides_then_clamps_to_the_absolute_cap() {
        // Early on, exp tracks now + IDLE. Past the cap it freezes at iat+ABSOLUTE.
        let iat = 1000;
        assert_eq!(slide_exp(iat, 2000), 2000 + SESSION_IDLE_TTL_SECS, "slides");
        let late = iat + SESSION_ABSOLUTE_TTL_SECS; // now at the cap boundary
        assert_eq!(
            slide_exp(iat, late),
            iat + SESSION_ABSOLUTE_TTL_SECS,
            "frozen at the absolute cap"
        );
    }

    #[test]
    fn tampered_mac_rejected() {
        let c = sign("tok", 0, 500, 1000);
        let mut bad = c.clone();
        bad.pop();
        bad.push(if c.ends_with('a') { 'b' } else { 'a' });
        assert!(
            !verify("tok", 0, &bad, 900),
            "a tampered MAC must not verify"
        );
    }

    #[test]
    fn a_tampered_iat_is_rejected() {
        // iat is in the MAC, so lengthening the absolute cap by editing it fails.
        let c = sign("tok", 0, 500, 1000);
        let forged = c.replacen("2.0.500.", "2.0.999.", 1);
        assert!(
            !verify("tok", 0, &forged, 900),
            "editing iat breaks the MAC"
        );
    }

    #[test]
    fn wrong_token_rejected() {
        let c = sign("tok", 0, 500, 1000);
        assert!(
            !verify("other", 0, &c, 900),
            "a different signing key must not verify"
        );
    }

    #[test]
    fn malformed_value_rejected() {
        assert!(!verify("tok", 0, "garbage", 900));
        assert!(
            !verify("tok", 0, "1.1000.deadbeef", 900),
            "the pre-epoch v1 format no longer verifies"
        );
        assert!(
            !verify("tok", 0, "2.0.1000.deadbeef", 900),
            "the 4-field (no-iat) format no longer verifies"
        );
        assert!(
            !verify("tok", 0, "3.0.500.1000.deadbeef", 900),
            "wrong version tag"
        );
        assert!(!verify("tok", 0, "2.notanumber.500.1000.deadbeef", 900));
        assert!(!verify("tok", 0, "2.0.500.notanumber.deadbeef", 900));
    }

    #[test]
    fn set_cookie_has_hardening_attrs() {
        let h = set_cookie_value("2.0.500.1000.abc");
        assert!(h.starts_with("ralphy_session=2.0.500.1000.abc"));
        assert!(h.contains("HttpOnly") && h.contains("SameSite=Strict") && h.contains("Path=/"));
        assert!(
            !h.contains("Secure"),
            "the cookie is deliberately not Secure"
        );
    }

    #[test]
    fn clear_cookie_expires_immediately() {
        let h = clear_cookie_value();
        assert!(h.contains("ralphy_session=;"), "empty value: {h}");
        assert!(h.contains("Max-Age=0"), "Max-Age=0: {h}");
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
