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
        remote_images: AtomicBool::new(false),
    }
}

#[test]
fn ct_eq_is_true_only_for_the_same_bytes_at_the_same_length() {
    assert!(ct_eq(b"token", b"token"));
    assert!(ct_eq(b"", b""));
    assert!(!ct_eq(b"token", b"Token"), "a first-byte difference");
    assert!(!ct_eq(b"token", b"tokem"), "a last-byte difference");
    assert!(!ct_eq(b"token", b"token2"), "a longer input, same prefix");
    assert!(!ct_eq(b"token2", b"token"), "a shorter input, same prefix");
    assert!(!ct_eq(b"token", b""), "an empty input");
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
        LoginOutcome::BadCredential | LoginOutcome::Replayed => {
            panic!("first login must succeed")
        }
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
        LoginOutcome::BadCredential | LoginOutcome::Replayed => panic!("login must succeed"),
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
