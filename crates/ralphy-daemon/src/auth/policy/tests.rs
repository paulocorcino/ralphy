use super::*;
use crate::auth::tests::{session_over, test_epoch};

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
fn compute_policy_gates_loopback_only_when_opted_in() {
    let seed = || totp::Seed::from_bytes(b"12345678901234567890".to_vec());
    let loop_ip: IpAddr = "127.0.0.1".parse().unwrap();

    // Default loopback: no gate.
    let p = compute_policy(loop_ip, None, None, None, false, false, test_epoch()).unwrap();
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
        false,
        test_epoch(),
    )
    .unwrap();
    assert!(
        matches!(p, AuthPolicy::Session(_)),
        "loopback gate engages with require-login + seed + token"
    );

    // Opted in but NO token to sign with → cannot gate, stays open (safe: it
    // is loopback; the enable route mints a token so this is transient).
    let p = compute_policy(loop_ip, None, Some(seed()), None, true, false, test_epoch()).unwrap();
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
    assert!(compute_policy(net_ip, None, None, None, false, false, test_epoch()).is_err());
    // Network + token, no seed → Bearer.
    let p = compute_policy(
        net_ip,
        Some("t".into()),
        None,
        None,
        false,
        false,
        test_epoch(),
    )
    .unwrap();
    assert!(matches!(p, AuthPolicy::Bearer(_)));
    // Network + token + seed → Session (unchanged §4 derived behavior).
    let p = compute_policy(
        net_ip,
        Some("t".into()),
        Some(seed()),
        None,
        false,
        false,
        test_epoch(),
    )
    .unwrap();
    assert!(matches!(p, AuthPolicy::Session(_)));
}

#[test]
fn compute_policy_requires_the_token_on_loopback_when_marked() {
    let seed = || totp::Seed::from_bytes(b"12345678901234567890".to_vec());
    let loop_ip: IpAddr = "127.0.0.1".parse().unwrap();

    let p = compute_policy(
        loop_ip,
        Some("k".into()),
        None,
        None,
        false,
        true,
        test_epoch(),
    )
    .unwrap();
    assert!(
        matches!(p, AuthPolicy::Bearer(ref t) if t == "k"),
        "marked loopback + token → Bearer"
    );

    let p = compute_policy(
        loop_ip,
        Some("k".into()),
        Some(seed()),
        None,
        false,
        true,
        test_epoch(),
    )
    .unwrap();
    assert!(
        matches!(p, AuthPolicy::Session(_)),
        "marked loopback + token + seed → Session"
    );

    let Err(err) = compute_policy(loop_ip, None, None, None, false, true, test_epoch()) else {
        panic!("marked loopback with no token must fail closed");
    };
    assert!(
        format!("{err:#}").contains("ralphy daemon require-token on"),
        "the error names the fix: {err:#}"
    );
    let empty = compute_policy(
        loop_ip,
        Some(String::new()),
        None,
        None,
        false,
        true,
        test_epoch(),
    );
    assert!(empty.is_err(), "an empty token counts as no token");

    let p = compute_policy(
        loop_ip,
        Some("k".into()),
        None,
        None,
        false,
        false,
        test_epoch(),
    )
    .unwrap();
    assert!(
        matches!(p, AuthPolicy::Localhost),
        "unmarked loopback stays open"
    );
}

#[test]
fn require_token_flag_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!require_token_enabled_in(dir.path()), "unset by default");
    set_require_token_in(dir.path(), true).unwrap();
    assert!(require_token_enabled_in(dir.path()), "set → on");
    assert!(
        !require_login_enabled_in(dir.path()),
        "require-token and require-login are separate files"
    );
    set_require_token_in(dir.path(), false).unwrap();
    assert!(!require_token_enabled_in(dir.path()), "cleared → off");
    // Idempotent clear.
    set_require_token_in(dir.path(), false).unwrap();
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
fn remote_images_flag_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!remote_images_enabled_in(dir.path()), "unset by default");
    set_remote_images_in(dir.path(), true).unwrap();
    assert!(remote_images_enabled_in(dir.path()), "set → on");
    assert!(
        !require_login_enabled_in(dir.path()),
        "the two flags are separate files"
    );
    set_remote_images_in(dir.path(), false).unwrap();
    assert!(!remote_images_enabled_in(dir.path()), "cleared → off");
    // Idempotent clear.
    set_remote_images_in(dir.path(), false).unwrap();
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
    let policy = AuthPolicy::for_bind("100.64.0.1".parse().unwrap(), Some("tok".into())).unwrap();
    assert!(matches!(policy, AuthPolicy::Bearer(t) if t == "tok"));
}
