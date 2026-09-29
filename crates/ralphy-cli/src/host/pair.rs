//! Pairing: the `ralphy host add`, `check` and `remove` flows over a
//! [`HostShell`](super::ssh::HostShell).

/// The first local port a tunnel takes. The range stays clear of the daemon's
/// default port, 7257.
pub(crate) const FIRST_TUNNEL_PORT: u16 = 7401;
const LAST_TUNNEL_PORT: u16 = 7499;

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

/// `text` without every line that holds `body` as one whole field. A key line
/// may start with options (`restrict,port-forwarding ssh-ed25519 …`), so the
/// body is not always the second field; a longer key that only contains the
/// body is kept. Line endings are kept byte for byte. `None` when no line
/// matched.
pub(crate) fn without_key_line(text: &str, body: &str) -> Option<String> {
    let mut kept = String::with_capacity(text.len());
    let mut dropped = false;
    for line in text.split_inclusive('\n') {
        if line.split_whitespace().any(|field| field == body) {
            dropped = true;
        } else {
            kept.push_str(line);
        }
    }
    dropped.then_some(kept)
}

#[cfg(test)]
mod tests;
