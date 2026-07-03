//! Emitter identity + id minting for the CloudEvents sink (ADR-0019).
//!
//! Every event carries a `data.emitter` object identifying the process that sent
//! it (version, user, host, os, pid, ip, tz) and a per-event ULID `id`; the run is
//! correlated by a single `runid` ULID minted once at process start. All fields
//! are best-effort and self-declared — attribution, never authentication
//! (docs/events.md).

use std::net::UdpSocket;
use std::path::Path;

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
        ip: local_ip(),
        tz: Local::now().format("%:z").to_string(),
    }
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

/// Mint a fresh per-event ULID (the envelope `id`: the dedup + sort key).
pub fn new_id() -> String {
    ulid::Ulid::new().to_string()
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
    use std::time::Duration;

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
    fn ids_differ_and_sort_by_time() {
        let a = new_id();
        // A short gap guarantees a later millisecond so the ULIDs sort by time.
        std::thread::sleep(Duration::from_millis(2));
        let b = new_id();
        assert_ne!(a, b, "two ids must differ");
        assert!(a < b, "later id must sort after earlier: {a} !< {b}");
    }
}
