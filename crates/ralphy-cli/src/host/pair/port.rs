//! The local port of a new peer tunnel: a pure choice among free loopback
//! ports that never takes the local daemon's own port.

/// The first local port a tunnel takes. The range stays clear of the daemon's
/// default port, 7257.
pub(crate) const FIRST_TUNNEL_PORT: u16 = 7401;
pub(super) const LAST_TUNNEL_PORT: u16 = 7499;

/// The local end of a new tunnel. A re-added daemon keeps its old port, so the
/// descriptor does not move; otherwise the first free port that is neither the
/// local daemon's nor another descriptor's.
pub(crate) fn choose_local_port(
    daemon_port: u16,
    taken: &[u16],
    keep: Option<u16>,
    is_free: impl Fn(u16) -> bool,
) -> Option<u16> {
    if let Some(port) = keep.filter(|p| *p != daemon_port) {
        return Some(port);
    }
    (FIRST_TUNNEL_PORT..=LAST_TUNNEL_PORT)
        .find(|p| *p != daemon_port && !taken.contains(p) && is_free(*p))
}

#[cfg(test)]
mod tests {
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
}
