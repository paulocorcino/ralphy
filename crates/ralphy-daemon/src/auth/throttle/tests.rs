use super::*;

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
