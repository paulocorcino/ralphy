//! The signed, stateless session cookie (issue #179, ADR-0032 §4 + amendment §B).
//! A browser login mints
//! `ralphy_session=3.<epoch>.<iat>.<exp>.<kind>.<hex HMAC-SHA1(token, "3|<epoch>|<iat>|<exp>|<kind>")>`;
//! every subsequent request is authorized by recomputing the MAC (constant-time)
//! and checking `exp > now`. No server-side store — the signing key is the daemon
//! access token (re-minting it invalidates outstanding cookies), and the
//! **session epoch** binds each cookie to the current epoch so a bump (logout,
//! re-mint, TOTP revoke, disabling require-login) invalidates every live cookie
//! instantly (amendment §B). Not `Secure`: the daemon never does TLS and rides
//! Tailscale/localhost.
//!
//! The `kind` is the session's length preset ([`SessionKind`], ADR-0032
//! amendment 2026-09-16): `s` (standard, 30 min idle / 12 h cap) or `r`
//! (remembered, 7 d idle / 30 d cap — the "keep me signed in" box). It sits in
//! the MAC so a standard cookie cannot be re-tagged into a remembered one.
//!
//! Format history: `1.<exp>.<mac>` (pre-epoch) and `2.<epoch>.<iat>.<exp>.<mac>`
//! (pre-kind) no longer verify — an old cookie fails closed and the browser
//! re-logs in once.

use hmac::{Hmac, Mac};
use sha1::Sha1;

use crate::auth;

/// The session cookie name.
pub const COOKIE_NAME: &str = "ralphy_session";

/// The standard ABSOLUTE session cap: 12h from login, never extended (amendment
/// §D). A hard ceiling even under continuous activity.
pub const SESSION_ABSOLUTE_TTL_SECS: u64 = 12 * 3600;

/// The standard IDLE window: a cookie unused for this long expires (amendment
/// §D). Each authorized request slides `exp` forward to `now + IDLE`, bounded by
/// the absolute cap — so 30 min of inactivity logs the operator out.
pub const SESSION_IDLE_TTL_SECS: u64 = 30 * 60;

/// Standard re-issue hysteresis: a slid cookie is only re-emitted when it moves
/// `exp` at least this far, so activity produces at most one `Set-Cookie` per
/// minute.
pub const SLIDE_MIN_SECS: u64 = 60;

/// The remembered ABSOLUTE cap: 30 days from login (amendment 2026-09-16).
pub const REMEMBERED_ABSOLUTE_TTL_SECS: u64 = 30 * 86_400;

/// The remembered IDLE window: 7 days unused (amendment 2026-09-16).
pub const REMEMBERED_IDLE_TTL_SECS: u64 = 7 * 86_400;

/// Remembered re-issue hysteresis: once an hour is plenty against a 7-day window.
pub const REMEMBERED_SLIDE_MIN_SECS: u64 = 3600;

/// The session's length preset, chosen at login and carried inside the cookie's
/// MAC. Two presets, deliberately not configurable (amendment 2026-09-16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    /// The default: 30 min idle, 12 h absolute.
    Standard,
    /// "Keep me signed in": 7 d idle, 30 d absolute.
    Remembered,
}

impl SessionKind {
    /// The idle window: unused for this long, the cookie expires.
    pub fn idle_secs(self) -> u64 {
        match self {
            SessionKind::Standard => SESSION_IDLE_TTL_SECS,
            SessionKind::Remembered => REMEMBERED_IDLE_TTL_SECS,
        }
    }

    /// The absolute cap from `iat`, never extended by activity.
    pub fn absolute_secs(self) -> u64 {
        match self {
            SessionKind::Standard => SESSION_ABSOLUTE_TTL_SECS,
            SessionKind::Remembered => REMEMBERED_ABSOLUTE_TTL_SECS,
        }
    }

    /// The minimum `exp` movement worth a re-issued `Set-Cookie`.
    pub fn slide_min_secs(self) -> u64 {
        match self {
            SessionKind::Standard => SLIDE_MIN_SECS,
            SessionKind::Remembered => REMEMBERED_SLIDE_MIN_SECS,
        }
    }

    /// The one-letter tag written into the cookie value.
    fn tag(self) -> &'static str {
        match self {
            SessionKind::Standard => "s",
            SessionKind::Remembered => "r",
        }
    }

    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "s" => Some(SessionKind::Standard),
            "r" => Some(SessionKind::Remembered),
            _ => None,
        }
    }
}

/// The validated claims carried by a session cookie: when it was issued (`iat`,
/// the anchor for the absolute cap), when it currently expires (`exp`, slid by
/// activity within the cap), and which length preset governs the slide.
pub struct Claims {
    pub iat: u64,
    pub exp: u64,
    pub kind: SessionKind,
}

/// The slid expiry for a `kind` cookie issued at `iat`, seen at `now`:
/// `now + IDLE` clamped to the absolute cap `iat + ABSOLUTE`. Monotonic
/// non-decreasing as `now` advances until it freezes at the cap.
pub fn slide_exp(kind: SessionKind, iat: u64, now: u64) -> u64 {
    (now + kind.idle_secs()).min(iat + kind.absolute_secs())
}

/// The signed message MAC'd under the token: version-tagged, epoch-bound, and
/// binding `iat`, `exp` and the kind so none can be tampered. Kept in one place
/// so `sign`/`verify` cannot drift apart.
fn mac_hex(token: &str, epoch: u64, iat: u64, exp: u64, kind: SessionKind) -> String {
    let mut mac =
        Hmac::<Sha1>::new_from_slice(token.as_bytes()).expect("HMAC accepts a key of any length");
    mac.update(format!("3|{epoch}|{iat}|{exp}|{}", kind.tag()).as_bytes());
    let bytes = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Mint a cookie value binding `epoch`, `iat`, `exp` and `kind` to `token`.
pub fn sign(token: &str, epoch: u64, kind: SessionKind, iat: u64, exp: u64) -> String {
    format!(
        "3.{epoch}.{iat}.{exp}.{}.{}",
        kind.tag(),
        mac_hex(token, epoch, iat, exp, kind)
    )
}

/// The validated claims of `value` for `token` at the current `epoch`, or `None`
/// when it is malformed, wrong-epoch, tampered, or expired at `now_unix`.
/// Requires the version tag `3`, the matching epoch, a known kind, and
/// `exp > now`. A cookie carrying any other epoch fails closed (invalidated by a
/// bump).
pub fn verify_claims(token: &str, epoch: u64, value: &str, now_unix: u64) -> Option<Claims> {
    let mut parts = value.splitn(6, '.');
    let (Some(ver), Some(epoch_str), Some(iat_str), Some(exp_str), Some(kind_str), Some(mac)) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    if ver != "3" {
        return None;
    }
    let cookie_epoch = epoch_str.parse::<u64>().ok()?;
    if cookie_epoch != epoch {
        return None;
    }
    let iat = iat_str.parse::<u64>().ok()?;
    let exp = exp_str.parse::<u64>().ok()?;
    let kind = SessionKind::from_tag(kind_str)?;
    if exp <= now_unix {
        return None;
    }
    if !auth::ct_eq(
        mac.as_bytes(),
        mac_hex(token, epoch, iat, exp, kind).as_bytes(),
    ) {
        return None;
    }
    Some(Claims { iat, exp, kind })
}

/// Whether `value` is a valid, unexpired cookie for `token` at `epoch`.
pub fn verify(token: &str, epoch: u64, value: &str, now_unix: u64) -> bool {
    verify_claims(token, epoch, value, now_unix).is_some()
}

/// The full `Set-Cookie` header value for a freshly minted or re-issued cookie.
/// `HttpOnly` (no JS access) + `SameSite=Strict` (no cross-site send); NOT
/// `Secure` (see module docs). `Max-Age` tracks the kind's idle window so the
/// browser also drops an idle cookie; server-side the absolute cap still bounds
/// it.
pub fn set_cookie_value(cookie: &str, kind: SessionKind) -> String {
    format!(
        "{COOKIE_NAME}={cookie}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        kind.idle_secs()
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

    const STD: SessionKind = SessionKind::Standard;
    const REM: SessionKind = SessionKind::Remembered;

    #[test]
    fn round_trip_verifies() {
        let c = sign("tok", 0, STD, 500, 1000);
        let claims = verify_claims("tok", 0, &c, 900).expect("a fresh cookie verifies before exp");
        assert_eq!((claims.iat, claims.exp), (500, 1000), "claims round-trip");
        assert_eq!(claims.kind, STD, "the kind round-trips");
    }

    #[test]
    fn a_remembered_cookie_round_trips_its_kind() {
        let c = sign("tok", 0, REM, 500, 1000);
        let claims = verify_claims("tok", 0, &c, 900).expect("verifies");
        assert_eq!(claims.kind, REM, "the remembered kind round-trips");
    }

    #[test]
    fn expired_cookie_rejected() {
        let c = sign("tok", 0, STD, 500, 1000);
        assert!(!verify("tok", 0, &c, 1000), "exp == now is expired");
        assert!(!verify("tok", 0, &c, 1500), "past exp is expired");
    }

    #[test]
    fn a_bumped_epoch_invalidates_the_cookie() {
        // The core of amendment §B: the same token + exp under a newer epoch no
        // longer verifies — a bump is instant, total logout.
        let c = sign("tok", 3, STD, 500, 1000);
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
        assert_eq!(
            slide_exp(STD, iat, 2000),
            2000 + SESSION_IDLE_TTL_SECS,
            "slides"
        );
        let late = iat + SESSION_ABSOLUTE_TTL_SECS; // now at the cap boundary
        assert_eq!(
            slide_exp(STD, iat, late),
            iat + SESSION_ABSOLUTE_TTL_SECS,
            "frozen at the absolute cap"
        );
    }

    #[test]
    fn a_remembered_cookie_slides_a_week_and_clamps_at_thirty_days() {
        let iat = 1000;
        assert_eq!(
            slide_exp(REM, iat, 2000),
            2000 + REMEMBERED_IDLE_TTL_SECS,
            "slides by the 7-day idle window"
        );
        // Deep into the session the 7-day slide would overshoot the 30-day cap.
        let late = iat + REMEMBERED_ABSOLUTE_TTL_SECS - 3600;
        assert_eq!(
            slide_exp(REM, iat, late),
            iat + REMEMBERED_ABSOLUTE_TTL_SECS,
            "frozen at the 30-day cap"
        );
    }

    #[test]
    fn tampered_mac_rejected() {
        let c = sign("tok", 0, STD, 500, 1000);
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
        let c = sign("tok", 0, STD, 500, 1000);
        let forged = c.replacen("3.0.500.", "3.0.999.", 1);
        assert!(
            !verify("tok", 0, &forged, 900),
            "editing iat breaks the MAC"
        );
    }

    #[test]
    fn a_retagged_kind_is_rejected() {
        // The kind is in the MAC: a standard cookie cannot be promoted to a
        // remembered one by flipping its tag.
        let c = sign("tok", 0, STD, 500, 1000);
        let forged = c.replacen(".s.", ".r.", 1);
        assert_ne!(c, forged, "the tag was found and flipped");
        assert!(
            !verify("tok", 0, &forged, 900),
            "editing the kind breaks the MAC"
        );
    }

    #[test]
    fn wrong_token_rejected() {
        let c = sign("tok", 0, STD, 500, 1000);
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
            !verify("tok", 0, "2.0.500.1000.deadbeef", 900),
            "the pre-kind v2 format no longer verifies"
        );
        assert!(
            !verify("tok", 0, "3.0.500.1000.deadbeef", 900),
            "the 5-field (no-kind) format does not verify"
        );
        assert!(
            !verify("tok", 0, "3.0.500.1000.x.deadbeef", 900),
            "an unknown kind tag is rejected"
        );
        assert!(
            !verify("tok", 0, "4.0.500.1000.s.deadbeef", 900),
            "wrong version tag"
        );
        assert!(!verify("tok", 0, "3.notanumber.500.1000.s.deadbeef", 900));
        assert!(!verify("tok", 0, "3.0.500.notanumber.s.deadbeef", 900));
    }

    #[test]
    fn set_cookie_has_hardening_attrs() {
        let h = set_cookie_value("3.0.500.1000.s.abc", STD);
        assert!(h.starts_with("ralphy_session=3.0.500.1000.s.abc"));
        assert!(h.contains("HttpOnly") && h.contains("SameSite=Strict") && h.contains("Path=/"));
        assert!(
            !h.contains("Secure"),
            "the cookie is deliberately not Secure"
        );
    }

    #[test]
    fn set_cookie_max_age_follows_the_kind() {
        let std = set_cookie_value("3.0.500.1000.s.abc", STD);
        assert!(std.ends_with("Max-Age=1800"), "standard idle: {std}");
        let rem = set_cookie_value("3.0.500.1000.r.abc", REM);
        assert!(rem.ends_with("Max-Age=604800"), "remembered idle: {rem}");
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
