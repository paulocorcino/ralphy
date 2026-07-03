//! Emitter identity + id minting for the CloudEvents sink (ADR-0019).
//!
//! Every event carries a `data.emitter` object identifying the process that sent
//! it (version, user, host, os, pid, ip, tz) and a per-event ULID `id`; the run is
//! correlated by a single `runid` ULID minted once at process start. All fields
//! are best-effort and self-declared — attribution, never authentication
//! (docs/events.md).

use std::net::UdpSocket;
use std::path::Path;
use std::time::Duration;

use chrono::Local;
use serde::Serialize;

/// The reserved `data.emitter` identity block carried on every event (ADR-0019 §3).
/// Every field is best-effort: `user` is empty when `git config user.email` is
/// unset, `ip` degrades to `0.0.0.0` when the host is offline. None of these are
/// ever used as a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Emitter {
    /// The Ralphy binary version — which contract vintage is emitting.
    pub version: String,
    /// `git config user.email` — attribution to a person (may be empty).
    pub user: String,
    /// Hostname of the machine the run works on.
    pub host: String,
    /// OS family: `windows` / `linux` / `macos` (`std::env::consts::OS`).
    pub os: String,
    /// Process id — which process among concurrent Ralphys on one host.
    pub pid: u32,
    /// Primary local IP (best-effort) — a network diagnostic, never a key.
    pub ip: String,
    /// Local timezone as a fixed UTC offset (e.g. `-03:00`) — parsers accept both
    /// this and an IANA name (docs/events.md).
    pub tz: String,
}

/// Detect the emitter identity for this process and repo. Every probe is
/// best-effort and falls back to a safe default rather than failing.
pub fn detect(repo_root: &Path) -> Emitter {
    Emitter {
        version: env!("CARGO_PKG_VERSION").to_string(),
        user: ralphy_core::git::user_email(repo_root).unwrap_or_default(),
        host: hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_default(),
        os: std::env::consts::OS.to_string(),
        pid: std::process::id(),
        // Best-effort public egress IP (#96); falls back to the primary local IP,
        // then `0.0.0.0` when every probe fails (offline/firewalled host).
        ip: public_ip().unwrap_or_else(local_ip),
        tz: Local::now().format("%:z").to_string(),
    }
}

/// The endpoints returning the caller's public IP as a bare line (raw IP → trim),
/// probed in order (#96): a mix of providers so one outage does not blind the run.
const IP_PROBES_RAW: [&str; 3] = [
    "https://checkip.amazonaws.com/",
    "http://checkip.global.api.aws/",
    "http://icanhazip.com/",
];

/// The Cloudflare trace endpoint, whose body is a `key=value` block carrying an
/// `ip=<addr>` line — the fallback when every raw-IP probe fails (#96).
const IP_PROBE_TRACE: &str = "https://www.cloudflare.com/cdn-cgi/trace";

/// The overall wall-clock budget for the whole probe sequence (#96), bounding the
/// worst case (every endpoint blackholed) so the synchronous run-start ctx build is
/// never held for more than one in-flight request past this cap.
const IP_PROBE_BUDGET: Duration = Duration::from_secs(4);

/// Detect the host's **public egress** IP, best-effort (#96): GET each raw-IP
/// endpoint in order (trim + validate the body), then the Cloudflare trace endpoint
/// (extract the `ip=` line). `None` when every probe fails or is unreachable — the
/// caller then falls back to the local IP. Each request is bounded to ~2s AND the
/// whole sequence to [`IP_PROBE_BUDGET`], so a wedged endpoint cannot hang run start;
/// the first valid answer wins.
fn public_ip() -> Option<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(2))
        .build();
    let deadline = std::time::Instant::now() + IP_PROBE_BUDGET;
    for url in IP_PROBES_RAW {
        if std::time::Instant::now() >= deadline {
            return None;
        }
        if let Ok(resp) = agent.get(url).call() {
            if let Some(ip) = resp.into_string().ok().and_then(|b| parse_raw_ip(&b)) {
                return Some(ip);
            }
        }
    }
    if std::time::Instant::now() < deadline {
        if let Ok(resp) = agent.get(IP_PROBE_TRACE).call() {
            if let Some(ip) = resp
                .into_string()
                .ok()
                .and_then(|b| parse_cloudflare_trace(&b))
            {
                return Some(ip);
            }
        }
    }
    None
}

/// Parse a raw-IP probe body: trim surrounding whitespace and accept it only when it
/// is a valid IP address (so a captive-portal HTML page or empty body yields `None`).
fn parse_raw_ip(body: &str) -> Option<String> {
    let s = body.trim();
    s.parse::<std::net::IpAddr>().ok().map(|_| s.to_string())
}

/// Extract the IP from a Cloudflare `cdn-cgi/trace` body: find the `ip=<addr>` line
/// and validate the address via [`parse_raw_ip`].
fn parse_cloudflare_trace(body: &str) -> Option<String> {
    body.lines()
        .find_map(|l| l.trim().strip_prefix("ip="))
        .and_then(parse_raw_ip)
}

/// The host's primary local IP, discovered with the classic connect-a-UDP-socket
/// trick: `connect` on a datagram socket only selects the outbound interface (no
/// packet is sent), so `local_addr` reports the IP the OS would route from.
/// Best-effort — `0.0.0.0` when the host is offline or the probe fails.
fn local_ip() -> String {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|sock| {
            sock.connect("8.8.8.8:80")?;
            Ok(sock.local_addr()?.ip().to_string())
        })
        .unwrap_or_else(|_| "0.0.0.0".to_string())
}

std::thread_local! {
    /// A per-thread monotonic ULID generator for the envelope `id`. The contract
    /// (docs/events.md) tells consumers to "order by `id`" because `time` is only
    /// second-resolution — but plain `Ulid::new()` draws fresh random bits each call,
    /// so two ids minted in the same millisecond would sort by randomness, shuffling
    /// a same-ms lifecycle burst (`issue.started` → `planning` → `executing`). A
    /// monotonic generator increments the random component within a millisecond, so
    /// ids stay emission-ordered. All `id` minting happens on the single sender
    /// thread, so a thread-local generator is exactly the right sequence.
    static ID_GEN: std::cell::RefCell<ulid::Generator> =
        const { std::cell::RefCell::new(ulid::Generator::new()) };
}

/// Mint a fresh per-event ULID (the envelope `id`: the dedup + sort key), monotonic
/// within a millisecond so same-ms events sort by emission order. Falls back to a
/// plain ULID on the (practically impossible) same-ms overflow.
pub fn new_id() -> String {
    ID_GEN
        .with(|g| {
            g.borrow_mut()
                .generate()
                .unwrap_or_else(|_| ulid::Ulid::new())
        })
        .to_string()
}

/// Mint the process `runid` ULID (the run-correlation key), once at process start.
pub fn new_runid() -> String {
    ulid::Ulid::new().to_string()
}

/// The CloudEvents `source` for a repo slug: `ralphy/<owner>/<repo>`.
pub fn source(slug: &str) -> String {
    format!("ralphy/{slug}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_raw_ip_trims_and_validates() {
        // A bare IP with surrounding whitespace/newline is trimmed and accepted.
        assert_eq!(
            parse_raw_ip("203.0.113.7\n"),
            Some("203.0.113.7".to_string())
        );
        assert_eq!(
            parse_raw_ip("  2001:db8::1  "),
            Some("2001:db8::1".to_string())
        );
        // Non-IP bodies (a captive-portal page, an empty body) yield None so the
        // caller falls through to the next probe / the local-IP fallback.
        assert_eq!(parse_raw_ip("<html>nope</html>"), None);
        assert_eq!(parse_raw_ip(""), None);
    }

    #[test]
    fn parse_cloudflare_trace_extracts_ip_line() {
        let body = "fl=1\nh=www.cloudflare.com\nip=198.51.100.42\nts=1700000000\n";
        assert_eq!(
            parse_cloudflare_trace(body),
            Some("198.51.100.42".to_string())
        );
        // No `ip=` line → None (fall back to local IP).
        assert_eq!(parse_cloudflare_trace("fl=1\nh=x\n"), None);
        // A garbage `ip=` value is rejected by the IP validation.
        assert_eq!(parse_cloudflare_trace("ip=not-an-ip\n"), None);
    }

    #[test]
    fn public_ip_falls_back_to_local_when_probes_yield_nothing() {
        // The composed fallback the emitter uses: `public_ip().unwrap_or_else(local_ip)`
        // always yields a non-empty string (a real IP, or `0.0.0.0` when fully
        // offline) — never an empty `ip` field on the wire.
        let ip = public_ip().unwrap_or_else(local_ip);
        assert!(!ip.is_empty(), "ip must never be empty");
        assert!(
            ip.parse::<std::net::IpAddr>().is_ok(),
            "ip must be a valid address, got {ip}"
        );
    }

    #[test]
    fn detect_yields_non_empty_core_fields() {
        let e = detect(Path::new("."));
        assert!(!e.version.is_empty(), "version empty");
        assert!(!e.host.is_empty(), "host empty");
        assert!(!e.os.is_empty(), "os empty");
        assert!(e.pid > 0, "pid was {}", e.pid);
        // The OS family is one of the plain forms the contract lists.
        assert!(
            ["windows", "linux", "macos"].contains(&e.os.as_str()),
            "unexpected os form: {}",
            e.os
        );
        // The tz is a fixed offset like `-03:00` / `+00:00`.
        assert!(
            e.tz.starts_with('+') || e.tz.starts_with('-'),
            "tz not an offset: {}",
            e.tz
        );
        // The emitter serializes to an object carrying all seven fields.
        let v = serde_json::to_value(&e).unwrap();
        for key in ["version", "user", "host", "os", "pid", "ip", "tz"] {
            assert!(v.get(key).is_some(), "emitter missing {key}: {v}");
        }
    }

    #[test]
    fn source_prefixes_slug() {
        assert_eq!(source("o/r"), "ralphy/o/r");
    }

    #[test]
    fn ids_are_monotonic_even_within_a_millisecond() {
        // No sleep: a burst of ids minted back-to-back (very likely within one
        // millisecond) must still differ and sort by emission order — the property
        // the "order by id" contract depends on. A plain `Ulid::new()` burst would
        // fail this (random bits, not monotonic).
        let ids: Vec<String> = (0..50).map(|_| new_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(
            ids, sorted,
            "ids must already be in ascending (emission) order"
        );
        // All distinct.
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "ids must be unique");
        // And still sort across a millisecond boundary.
        let a = new_id();
        std::thread::sleep(Duration::from_millis(2));
        let b = new_id();
        assert!(a < b, "later id must sort after earlier: {a} !< {b}");
    }
}
