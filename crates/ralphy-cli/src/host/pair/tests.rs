use super::*;

#[test]
fn choose_local_port_skips_the_daemon_port() {
    assert_eq!(choose_local_port(7401, &[], None, |_| true), Some(7402));
}

#[test]
fn choose_local_port_skips_taken_and_busy() {
    assert_eq!(
        choose_local_port(7257, &[7401], None, |p| p != 7402),
        Some(7403)
    );
    assert_eq!(choose_local_port(7257, &[], None, |_| false), None);
}

#[test]
fn choose_local_port_keeps_a_readd() {
    assert_eq!(
        choose_local_port(7257, &[7410], Some(7410), |_| false),
        Some(7410)
    );
    assert_eq!(
        choose_local_port(7410, &[], Some(7410), |_| true),
        Some(7401),
        "an old port that is now the daemon's is not kept"
    );
}

const KEYS: &str = "ssh-ed25519 OTHER me@laptop\n\
restrict,port-forwarding ssh-ed25519 BODY ralphy-peer@anvil\n\
ssh-ed25519 BODY2 ralphy-peer@forge\n";

#[test]
fn without_key_line_removes_exactly_ours() {
    assert_eq!(
        without_key_line(KEYS, "BODY").as_deref(),
        Some("ssh-ed25519 OTHER me@laptop\nssh-ed25519 BODY2 ralphy-peer@forge\n")
    );
}

#[test]
fn without_key_line_keeps_crlf() {
    let text =
        "ssh-ed25519 OTHER me@laptop\r\nssh-ed25519 BODY ralphy-peer@anvil\r\nssh-rsa LAST x";
    assert_eq!(
        without_key_line(text, "BODY").as_deref(),
        Some("ssh-ed25519 OTHER me@laptop\r\nssh-rsa LAST x")
    );
}

#[test]
fn without_key_line_none_when_absent() {
    assert_eq!(without_key_line(KEYS, "BOD"), None);
    assert_eq!(without_key_line("", "BODY"), None);
}
