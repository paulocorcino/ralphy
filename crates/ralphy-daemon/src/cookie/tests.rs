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
        Some("xyz".to_owned())
    );
    assert_eq!(
        from_cookie_header(Some("ralphy_session=lone")),
        Some("lone".into())
    );
    assert_eq!(from_cookie_header(Some("other=1")), None);
    assert_eq!(from_cookie_header(None), None);
}
