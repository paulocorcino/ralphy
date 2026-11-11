//! The host spellings that name the daemon, for the `Origin` and `Host` checks.

use std::net::IpAddr;

/// The host part of a `Host` header, minus any port. Handles the bracketed IPv6
/// form (`[::1]:7257`), where a plain `rsplit_once(':')` would cut the address.
pub(crate) fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

/// The host spellings that name the daemon ITSELF for a given bind — the names a
/// browser can legitimately have in its address bar, and therefore the only ones
/// allowed in `Origin`/`Host`.
///
/// Derived from the bind, not hardcoded: §4 designs a non-loopback bind (a
/// Tailscale interface IP) as the remote-access path, so pinning this to loopback
/// refuses the operator's own browser on every route. A specific non-loopback
/// bind makes loopback UNREACHABLE, so it is dropped from the set — naming an
/// address the daemon does not answer on would only widen the allowlist.
///
/// A wildcard bind (`0.0.0.0`/`::`) answers on interfaces we cannot enumerate
/// without probing the OS; loopback is included and everything else must be
/// declared. That is the `configured` list: an operator-supplied allowlist of
/// names they reach the daemon by (`MagicDNS`, a reverse-proxy hostname). It is
/// also the DNS-rebinding boundary — an attacker domain that re-resolves to the
/// daemon's IP is refused unless the operator named it.
pub(super) fn allowed_host_set(bound_ip: IpAddr, configured: &[String]) -> Vec<String> {
    let mut hosts = Vec::new();
    if bound_ip.is_loopback() || bound_ip.is_unspecified() {
        hosts.push("127.0.0.1".to_owned());
        hosts.push("localhost".to_owned());
        hosts.push("::1".to_owned());
    }
    if !bound_ip.is_loopback() && !bound_ip.is_unspecified() {
        hosts.push(bound_ip.to_string());
    }
    hosts.extend(declared_host_set(configured));
    hosts
}

/// The operator-declared hosts, normalized, with blanks dropped.
pub(super) fn declared_host_set(configured: &[String]) -> Vec<String> {
    configured
        .iter()
        .map(|h| normalize_declared(h))
        .filter(|h| !h.is_empty())
        .collect()
}

/// Normalize one declared host: drop the scheme, the path, and the port, then
/// lowercase.
///
/// Accepting a whole URL is not politeness, it is the failure mode. Tunnel CLIs
/// print the endpoint as `https://<host>/`, so pasting that is the obvious
/// gesture — and a bare `host_name` would take everything before the LAST colon
/// and silently declare the host `https`. The operator then gets a blanket 403
/// with nothing naming the cause.
pub(super) fn normalize_declared(raw: &str) -> String {
    let host = raw.trim();
    let host = host.split_once("://").map_or(host, |(_, rest)| rest);
    let host = host.split('/').next().unwrap_or(host);
    host_name(host).to_ascii_lowercase()
}

/// A host spelled for the authority part of an origin: IPv6 needs brackets.
pub(super) fn origin_host(host: &str) -> String {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(ip)) => format!("[{ip}]"),
        _ => host.to_owned(),
    }
}

/// Whether two host strings name the same host. IPs compare as parsed addresses
/// so alternate spellings of one address agree; names compare ASCII-caselessly.
pub(super) fn host_eq(a: &str, b: &str) -> bool {
    match (a.parse::<IpAddr>(), b.parse::<IpAddr>()) {
        (Ok(a), Ok(b)) => a == b,
        (Err(_), Err(_)) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
}
