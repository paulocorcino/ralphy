//! The signed, stateless session cookie (issue #179, ADR-0032 §4 + amendment §B).
//! A browser login mints
//! `ralphy_session=4.<epoch>.<iat>.<exp>.<kind>.<login>.<mac>`, where `<mac>` is
//! the hex HMAC-SHA1 under the token of `4|<epoch>|<iat>|<exp>|<kind>|<login>`;
//! every subsequent request is authorized by recomputing the MAC (constant-time)
//! and checking `exp > now`. No server-side store — the signing key is the daemon
//! access token (re-minting it invalidates outstanding cookies), and the
//! **session epoch** binds each cookie to the current epoch so a bump (logout,
//! re-mint, TOTP revoke, disabling require-login) invalidates every live cookie
//! instantly (amendment §B). `Secure` only when the login itself arrived over
//! https — signalled by a TLS-terminating front's `X-Forwarded-Proto: https`
//! (measured: dev tunnels forwards it) — never unconditionally: the daemon
//! does no TLS itself, and a `127.0.0.1` bind in a plain-http browser must
//! keep working (ADR-0032 audit amendment, F6).
//!
//! The `kind` is the session's length preset ([`SessionKind`], ADR-0032
//! amendment 2026-09-16): `s` (standard, 30 min idle / 12 h cap) or `r`
//! (remembered, 7 d idle / 30 d cap — the "keep me signed in" box). It sits in
//! the MAC so a standard cookie cannot be re-tagged into a remembered one.
//!
//! The `login` is the [`LoginId`] minted at login (issue #717): every cookie of
//! one login carries the same ID, and a slide keeps it. It sits in the MAC, so
//! the holder of a stolen cookie cannot swap it for another ID.
//!
//! Format history: `1.<exp>.<mac>` (pre-epoch), `2.<epoch>.<iat>.<exp>.<mac>`
//! (pre-kind) and `3.<epoch>.<iat>.<exp>.<kind>.<mac>` (pre-login-ID) no longer
//! verify — an old cookie fails closed and the browser re-logs in once.

use std::fmt;

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

/// The number of characters of a [`LoginId`] in its text form.
const LOGIN_ID_LEN: usize = 22;

/// The base64url alphabet (RFC 4648 §5), written without padding.
const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// One login's ID: 16 random bytes minted when the login succeeds, carried in
/// every cookie of that login. Its text form is 22 base64url characters.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LoginId([u8; 16]);

impl LoginId {
    /// A new random ID.
    pub fn mint() -> LoginId {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the OS CSPRNG must be available to mint a login ID");
        LoginId(bytes)
    }

    /// The ID written as `text`: exactly 22 base64url characters in the one
    /// form that `Display` writes, else `None`.
    pub fn parse(text: &str) -> Option<LoginId> {
        if text.len() != LOGIN_ID_LEN {
            return None;
        }
        let id = LoginId(from_base64url(text)?);
        // The last character holds 2 bits of the ID and 4 bits that must be 0:
        // only the canonical spelling is an ID.
        (id.to_string() == text).then_some(id)
    }
}

/// The ID as 22 base64url characters, without padding.
impl fmt::Display for LoginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&to_base64url(&self.0))
    }
}

impl fmt::Debug for LoginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LoginId({self})")
    }
}

/// `bytes` in base64url without padding.
fn to_base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 8).div_ceil(6));
    let mut acc: u32 = 0;
    let mut bits = 0;
    for &b in bytes {
        acc = ((acc << 8) | u32::from(b)) & 0xffff;
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            out.push(char::from(BASE64URL[((acc >> bits) & 63) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE64URL[((acc << (6 - bits)) & 63) as usize]));
    }
    out
}

/// The 16 bytes that 22 base64url characters decode to, or `None` when a
/// character is outside the alphabet or the length is not 16 bytes.
fn from_base64url(text: &str) -> Option<[u8; 16]> {
    let mut out = [0u8; 16];
    let mut len = 0;
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in text.bytes() {
        let value = BASE64URL.iter().position(|&a| a == c)?;
        acc = ((acc << 6) | value as u32) & 0xfff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            *out.get_mut(len)? = (acc >> bits) as u8;
            len += 1;
        }
    }
    (len == out.len()).then_some(out)
}

/// The validated claims carried by a session cookie: when it was issued (`iat`,
/// the anchor for the absolute cap), when it currently expires (`exp`, slid by
/// activity within the cap), which length preset governs the slide, and the
/// login the cookie belongs to.
pub struct Claims {
    pub iat: u64,
    pub exp: u64,
    pub kind: SessionKind,
    pub login: LoginId,
}

/// The slid expiry for a `kind` cookie issued at `iat`, seen at `now`:
/// `now + IDLE` clamped to the absolute cap `iat + ABSOLUTE`. Monotonic
/// non-decreasing as `now` advances until it freezes at the cap.
pub fn slide_exp(kind: SessionKind, iat: u64, now: u64) -> u64 {
    (now + kind.idle_secs()).min(iat + kind.absolute_secs())
}

/// The signed message MAC'd under the token: version-tagged, epoch-bound, and
/// binding `iat`, `exp`, the kind and the login ID so none can be tampered.
/// Kept in one place so `sign`/`verify` cannot drift apart.
fn mac_hex(token: &str, epoch: u64, claims: &Claims) -> String {
    let Claims {
        iat,
        exp,
        kind,
        login,
    } = claims;
    let mut mac =
        Hmac::<Sha1>::new_from_slice(token.as_bytes()).expect("HMAC accepts a key of any length");
    mac.update(format!("4|{epoch}|{iat}|{exp}|{}|{login}", kind.tag()).as_bytes());
    let bytes = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Mint a cookie value binding `epoch`, `iat`, `exp`, `kind` and `login` to
/// `token`.
pub fn sign(
    token: &str,
    epoch: u64,
    kind: SessionKind,
    iat: u64,
    exp: u64,
    login: LoginId,
) -> String {
    let claims = Claims {
        iat,
        exp,
        kind,
        login,
    };
    format!(
        "4.{epoch}.{iat}.{exp}.{}.{login}.{}",
        kind.tag(),
        mac_hex(token, epoch, &claims)
    )
}

/// The validated claims of `value` for `token` at the current `epoch`, or `None`
/// when it is malformed, wrong-epoch, tampered, or expired at `now_unix`.
/// Requires the version tag `4`, the matching epoch, a known kind, a well-formed
/// login ID, and `exp > now`. A cookie carrying any other epoch fails closed
/// (invalidated by a bump).
pub fn verify_claims(token: &str, epoch: u64, value: &str, now_unix: u64) -> Option<Claims> {
    let (cookie_epoch, claims, mac) = parse_value(value)?;
    if cookie_epoch != epoch || claims.exp <= now_unix {
        return None;
    }
    let expected = mac_hex(token, epoch, &claims);
    auth::ct_eq(mac.as_bytes(), expected.as_bytes()).then_some(claims)
}

/// The epoch, the claims and the MAC written in a format-4 cookie value, before
/// any check of the MAC.
fn parse_value(value: &str) -> Option<(u64, Claims, &str)> {
    let fields: Vec<&str> = value.splitn(7, '.').collect();
    let ["4", epoch, iat, exp, kind, login, mac] = fields[..] else {
        return None;
    };
    let claims = Claims {
        iat: iat.parse().ok()?,
        exp: exp.parse().ok()?,
        kind: SessionKind::from_tag(kind)?,
        login: LoginId::parse(login)?,
    };
    Some((epoch.parse().ok()?, claims, mac))
}

/// Whether `value` is a valid, unexpired cookie for `token` at `epoch`.
pub fn verify(token: &str, epoch: u64, value: &str, now_unix: u64) -> bool {
    verify_claims(token, epoch, value, now_unix).is_some()
}

/// The full `Set-Cookie` header value for a freshly minted or re-issued cookie.
/// `HttpOnly` (no JS access) + `SameSite=Strict` (no cross-site send); NOT
/// `Secure` — the plain-http default (see module docs and
/// [`set_cookie_value_with`]). `Max-Age` tracks the kind's idle window so the
/// browser also drops an idle cookie; server-side the absolute cap still bounds
/// it.
pub fn set_cookie_value(cookie: &str, kind: SessionKind) -> String {
    set_cookie_value_with(cookie, kind, false)
}

/// [`set_cookie_value`] with the `Secure` attribute when `secure` — the request
/// that mints or re-issues the cookie arrived over https, so the browser must
/// never send it back over http. The login and the idle-slide re-issue MUST
/// agree on `secure` for one session, or the slide would silently drop the
/// attribute (or add it and strand a plain-http browser).
pub fn set_cookie_value_with(cookie: &str, kind: SessionKind, secure: bool) -> String {
    format!(
        "{COOKIE_NAME}={cookie}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{}",
        kind.idle_secs(),
        secure_attr(secure)
    )
}

/// The `Set-Cookie` header value that CLEARS the session cookie: an empty value
/// with `Max-Age=0` so the browser drops it immediately. Same attributes as
/// [`set_cookie_value`] so the clear matches the original scope (issue #186).
pub fn clear_cookie_value() -> String {
    clear_cookie_value_with(false)
}

/// [`clear_cookie_value`] with `Secure` when the request arrived over https —
/// a clear must carry the attributes of the cookie it clears, or a browser
/// may refuse to let a plain `Set-Cookie` touch a `Secure` one.
pub fn clear_cookie_value_with(secure: bool) -> String {
    format!(
        "{COOKIE_NAME}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{}",
        secure_attr(secure)
    )
}

fn secure_attr(secure: bool) -> &'static str {
    if secure {
        "; Secure"
    } else {
        ""
    }
}

/// Extract the `ralphy_session` value from a `Cookie:` request header, or `None`
/// when absent. Handles a multi-cookie header (`a=1; ralphy_session=…; b=2`).
pub fn from_cookie_header(header: Option<&str>) -> Option<String> {
    named_value(header, COOKIE_NAME).map(str::to_string)
}

/// The value of the cookie `name` in a `Cookie:` request header, or `None`.
pub(crate) fn named_value<'h>(header: Option<&'h str>, name: &str) -> Option<&'h str> {
    header?.split(';').find_map(|pair| {
        pair.trim()
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STD: SessionKind = SessionKind::Standard;
    const REM: SessionKind = SessionKind::Remembered;

    /// The login ID of 16 zero bytes, in its text form.
    const ZERO_ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";

    fn sign(token: &str, epoch: u64, kind: SessionKind, iat: u64, exp: u64) -> String {
        super::sign(token, epoch, kind, iat, exp, LoginId::mint())
    }

    #[test]
    fn round_trip_verifies() {
        let c = sign("tok", 0, STD, 500, 1000);
        let claims = verify_claims("tok", 0, &c, 900).expect("a fresh cookie verifies before exp");
        assert_eq!((claims.iat, claims.exp), (500, 1000), "claims round-trip");
        assert_eq!(claims.kind, STD, "the kind round-trips");
    }

    #[test]
    fn a_login_id_round_trips_through_the_cookie() {
        let login = LoginId::mint();
        let c = super::sign("tok", 0, STD, 500, 1000, login);
        let claims = verify_claims("tok", 0, &c, 900).expect("a fresh cookie verifies");
        assert_eq!(claims.login, login);
        assert_eq!(LoginId::parse(&login.to_string()), Some(login));
    }

    /// Cookies as a browser holds them, with each MAC from Python's
    /// `hmac.new(token, b"4|7|1700000000|1700001800|<kind>|AAAAAAAAAAAAAAAAAAAAAA",
    /// hashlib.sha1)`, not from the crate under test. A dependency update that
    /// changes the MAC or its hex form fails here, instead of logging out every
    /// browser (#443).
    #[test]
    fn a_v4_cookie_matches_an_independent_hmac() {
        let token = "0123456789abcdef".repeat(4);
        let login = LoginId::parse(ZERO_ID).expect("22 zero characters are an ID");
        for (kind, cookie) in [
            (
                STD,
                "4.7.1700000000.1700001800.s.AAAAAAAAAAAAAAAAAAAAAA.\
                 4202cda714ddf4ff9e7f946d4449f218a875d2cb",
            ),
            (
                REM,
                "4.7.1700000000.1700001800.r.AAAAAAAAAAAAAAAAAAAAAA.\
                 c2481af5473ae872b4158b1173dfd7af58ad8086",
            ),
        ] {
            let signed = super::sign(&token, 7, kind, 1_700_000_000, 1_700_001_800, login);
            assert_eq!(signed, cookie);
            let claims = verify_claims(&token, 7, cookie, 1_700_000_900).expect("verifies");
            assert_eq!((claims.kind, claims.login), (kind, login));
        }
    }

    /// The text form is base64url without padding, as Python's
    /// `base64.urlsafe_b64encode(bytes(range(16))).rstrip(b"=")` writes it.
    #[test]
    fn a_login_id_is_written_in_base64url() {
        let id = LoginId::parse("AAECAwQFBgcICQoLDA0ODw").expect("a valid ID");
        assert_eq!(id.0, core::array::from_fn::<u8, 16, _>(|i| i as u8));
        assert_eq!(id.to_string(), "AAECAwQFBgcICQoLDA0ODw");
    }

    #[test]
    fn a_tampered_login_id_is_rejected() {
        let c = super::sign(
            "tok",
            0,
            STD,
            500,
            1000,
            LoginId::parse(ZERO_ID).expect("an ID"),
        );
        let edited = c.replacen(ZERO_ID, "AAAAAAAAAAAAAAAAAAAAAQ", 1);
        assert_ne!(c, edited, "the ID was found and edited");
        assert!(
            !verify("tok", 0, &edited, 900),
            "an edited ID breaks the MAC"
        );
        for bad in [
            "AAAAAAAAAAAAAAAAAAAAA",   // 21 characters
            "AAAAAAAAAAAAAAAAAAAAAAA", // 23 characters
            "AAAAAAAAAAAAAAAAAAAAA+",  // outside base64url
            "AAAAAAAAAAAAAAAAAAAAAB",  // the 4 unused bits are not 0
        ] {
            assert_eq!(LoginId::parse(bad), None, "{bad}");
            let malformed = c.replacen(ZERO_ID, bad, 1);
            assert!(!verify("tok", 0, &malformed, 900), "{malformed}");
        }
    }

    /// A format-3 cookie, as a browser held it before the login ID: its MAC is
    /// right for its own format, and it still fails closed.
    #[test]
    fn a_v3_cookie_no_longer_verifies() {
        let token = "0123456789abcdef".repeat(4);
        let v3 = "3.7.1700000000.1700001800.s.31965787eada68ef54fd63e069bd9f4fff61796c";
        assert!(verify_claims(&token, 7, v3, 1_700_000_900).is_none());
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
        let forged = c.replacen("4.0.500.", "4.0.999.", 1);
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
            !verify("tok", 0, "4.0.500.1000.s.deadbeef", 900),
            "the 6-field (no-login-ID) format does not verify"
        );
        assert!(
            !verify(
                "tok",
                0,
                "4.0.500.1000.x.AAAAAAAAAAAAAAAAAAAAAA.deadbeef",
                900
            ),
            "an unknown kind tag is rejected"
        );
        assert!(
            !verify(
                "tok",
                0,
                "5.0.500.1000.s.AAAAAAAAAAAAAAAAAAAAAA.deadbeef",
                900
            ),
            "wrong version tag"
        );
        assert!(!verify(
            "tok",
            0,
            "4.notanumber.500.1000.s.AAAAAAAAAAAAAAAAAAAAAA.deadbeef",
            900
        ));
        assert!(!verify(
            "tok",
            0,
            "4.0.500.notanumber.s.AAAAAAAAAAAAAAAAAAAAAA.deadbeef",
            900
        ));
    }

    #[test]
    fn set_cookie_has_hardening_attrs() {
        let h = set_cookie_value("3.0.500.1000.s.abc", STD);
        assert!(h.starts_with("ralphy_session=3.0.500.1000.s.abc"));
        assert!(h.contains("HttpOnly") && h.contains("SameSite=Strict") && h.contains("Path=/"));
        assert!(
            !h.contains("Secure"),
            "the cookie is not Secure by default (plain-http loopback)"
        );
        // Over https (a TLS-terminating front said so) it is, and so is the
        // clear that has to match it.
        let h = set_cookie_value_with("3.0.500.1000.s.abc", STD, true);
        assert!(h.ends_with("; Secure"), "{h}");
        assert!(h.contains("HttpOnly") && h.contains("SameSite=Strict"));
        assert!(!clear_cookie_value().contains("Secure"));
        assert!(clear_cookie_value_with(true).ends_with("; Secure"));
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
