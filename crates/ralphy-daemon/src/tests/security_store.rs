//! The security state and the stores behind it: TOTP, password and token.

use super::*;

#[test]
fn security_state_reflects_the_stores() {
    let dir = tempfile::tempdir().unwrap();
    // Empty store → every factor unset.
    let s = security_state_at(dir.path());
    assert!(!s.token_set && !s.password_set && !s.totp_enrolled && !s.require_login);
    assert!(!s.remote_images, "remote images are off by default");
    // Writing a seed flips totp_enrolled — but require_login is now the
    // PERSISTED opt-in flag (amendment §A), NOT derived from the seed.
    totp::save_seed_to(&totp::generate_seed(), &totp::seed_path_in(dir.path())).unwrap();
    let s = security_state_at(dir.path());
    assert!(
        s.totp_enrolled && !s.require_login,
        "seed → enrolled, but require_login stays off until opted in"
    );
    // Setting the flag flips require_login on its own.
    auth::set_require_login_in(dir.path(), true).unwrap();
    assert!(
        security_state_at(dir.path()).require_login,
        "the flag drives require_login"
    );
    auth::set_remote_images_in(dir.path(), true).unwrap();
    let s = security_state_at(dir.path());
    assert!(s.remote_images, "the flag drives remote_images");
    assert!(
        !s.token_set && !s.password_set,
        "the two flags set no other factor"
    );
}

#[test]
fn enroll_totp_is_mint_once_with_ralphy_uri() {
    let dir = tempfile::tempdir().unwrap();
    let (uri, minted) = enroll_totp_at(dir.path()).unwrap();
    assert!(minted, "first enrol mints");
    assert!(
        uri.starts_with("otpauth://totp/ralphy:"),
        "the real provisioning URI; got {uri}"
    );
    let secret_of = |u: &str| {
        u.split("secret=")
            .nth(1)
            .and_then(|s| s.split('&').next())
            .unwrap()
            .to_string()
    };
    let (uri2, minted2) = enroll_totp_at(dir.path()).unwrap();
    assert!(!minted2, "second enrol does not re-mint");
    assert_eq!(secret_of(&uri), secret_of(&uri2), "same secret returned");
}

#[test]
fn enroll_then_confirm_arms_the_live_seed() {
    let dir = tempfile::tempdir().unwrap();
    // Seed the PENDING slot with the RFC vector so we know a valid code.
    totp::save_seed_to(
        &totp::Seed::from_bytes(b"12345678901234567890".to_vec()),
        &totp::pending_seed_path_in(dir.path()),
    )
    .unwrap();
    // Pending enrolment does not count as enrolled yet.
    assert!(
        !security_state_at(dir.path()).totp_enrolled,
        "a pending seed is not enrolled"
    );
    // A wrong code arms nothing.
    assert!(!confirm_totp_at(dir.path(), "999999", 59).unwrap());
    assert!(!security_state_at(dir.path()).totp_enrolled);
    // The RFC vector code (T=59 → 287082) confirms and arms the live seed.
    assert!(confirm_totp_at(dir.path(), "287082", 59).unwrap());
    assert!(
        security_state_at(dir.path()).totp_enrolled,
        "confirming arms TOTP"
    );
    // Revoke clears both live and (already-consumed) pending.
    revoke_totp_at(dir.path()).unwrap();
    assert!(!security_state_at(dir.path()).totp_enrolled);
}

#[test]
fn set_password_round_trips_set_then_clear() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        set_password_at(dir.path(), Some("pw")).unwrap(),
        "set → true"
    );
    assert!(
        security_state_at(dir.path()).password_set,
        "state reflects the set"
    );
    assert!(!set_password_at(dir.path(), None).unwrap(), "clear → false");
    assert!(
        !security_state_at(dir.path()).password_set,
        "state reflects the clear"
    );
}

#[test]
fn remint_token_yields_a_new_distinct_token() {
    let dir = tempfile::tempdir().unwrap();
    remint_token_at(dir.path()).unwrap();
    let first = auth::load_token_from(&auth::token_path_in(dir.path()))
        .unwrap()
        .expect("token written");
    assert_eq!(first.len(), 64, "64-hex token");
    remint_token_at(dir.path()).unwrap();
    let second = auth::load_token_from(&auth::token_path_in(dir.path()))
        .unwrap()
        .unwrap();
    assert_ne!(first, second, "remint rotates the token");
}

#[test]
fn require_login_gate_needs_an_enrolled_seed() {
    let dir = tempfile::tempdir().unwrap();
    // Enabling with no seed is refused; disabling is always Ok.
    assert!(require_login_at(dir.path(), true).is_err());
    assert!(require_login_at(dir.path(), false).is_ok());
    // With a seed enrolled, enabling is Ok.
    totp::save_seed_to(&totp::generate_seed(), &totp::seed_path_in(dir.path())).unwrap();
    assert!(require_login_at(dir.path(), true).is_ok());
}
