//! The releases fetch and its TTL disk cache (ADR-0056 §6).
//!
//! Shaped after `ralphy-pricing`'s models.dev refresh, for the same reasons:
//! a self-timestamped envelope, an atomic temp-plus-rename write, and a failure
//! that leaves the prior cache alone and returns without error. No network is
//! not an error — the caller shows what it last knew.

use std::path::Path;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::Release;

/// The project's releases, newest first.
///
/// Deliberately **not** `/releases/latest`: every tag carries a hyphen, so the
/// release workflow publishes it `--prerelease`, and the API excludes
/// pre-releases from `latest` — that endpoint answers 404 for this repo and
/// will keep doing so for as long as the project ships candidates (ADR-0056).
pub const DEFAULT_RELEASES_URL: &str =
    "https://api.github.com/repos/paulocorcino/ralphy/releases?per_page=10";

/// Cache freshness window: four reads a day is enough to notice a release cut
/// this morning, and far enough inside the unauthenticated rate limit (60 per
/// hour per address) that it cannot contribute to exhausting it.
pub const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Whole-request cap covering connect, TLS and body read: `timeout_read` resets
/// on every read, so a slow-drip body would otherwise run unbounded. Generous
/// compared to the pricing fetch because nothing waits on this one — it is a
/// background poll, never a step in a run.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ATTEMPTS: u32 = 2;
const RETRY_SLEEP: Duration = Duration::from_millis(500);

/// GitHub refuses an API request with no user agent, so this is required rather
/// than decorative. It names the product and nothing else: no version, no
/// identifier, no operating system — the request must stay unable to describe
/// the machine that made it (ADR-0056 §9).
const USER_AGENT: &str = "ralphy";

/// Options for a best-effort releases refresh. `url` is injectable so tests
/// point at a loopback listener and never touch the network.
pub struct RefreshOpts<'a> {
    pub url: &'a str,
    pub cache_path: &'a Path,
    pub ttl: Duration,
    pub force: bool,
    pub offline: bool,
}

impl<'a> RefreshOpts<'a> {
    /// The ordinary options: the real endpoint, the standard TTL, honouring the
    /// offline switch.
    pub fn new(cache_path: &'a Path) -> Self {
        RefreshOpts {
            url: DEFAULT_RELEASES_URL,
            cache_path,
            ttl: CACHE_TTL,
            force: false,
            offline: offline_env(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct CacheEnvelope {
    timestamp: String,
    releases: Vec<Release>,
}

/// When the cache is missing or stale (or `force`), GET `opts.url` and
/// atomically rewrite the cache. Offline, fresh, or any fetch failure leaves the
/// prior cache alone and returns without error — callers always fall through to
/// [`load`].
pub fn refresh_if_stale(opts: &RefreshOpts<'_>) {
    if opts.offline {
        return;
    }
    if !opts.force && cache_is_fresh(opts.cache_path, opts.ttl) {
        return;
    }
    match fetch_releases(opts.url) {
        Ok(releases) => {
            let envelope = CacheEnvelope {
                timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                releases,
            };
            match serde_json::to_vec_pretty(&envelope) {
                Ok(bytes) => {
                    if let Err(e) = atomic_write_cache(opts.cache_path, &bytes) {
                        warn!(
                            path = %opts.cache_path.display(),
                            error = %e,
                            "writing the release cache failed — keeping the prior one"
                        );
                    }
                }
                Err(e) => warn!(
                    error = %e,
                    "serializing the release cache failed — keeping the prior one"
                ),
            }
        }
        Err(e) => warn!(error = %e, "release fetch failed — using the prior cache"),
    }
}

/// The releases this machine last read. An absent, unreadable or malformed
/// cache is an empty list, never an error: not knowing is a normal state.
pub fn load(cache_path: &Path) -> Vec<Release> {
    let Ok(text) = std::fs::read_to_string(cache_path) else {
        return Vec::new();
    };
    serde_json::from_str::<CacheEnvelope>(&text)
        .map(|e| e.releases)
        .unwrap_or_default()
}

/// True when `RALPHY_RELEASE_OFFLINE` trims to `"1"`. Mirrors the pricing
/// crate's switch so an air-gapped operator turns off every outbound read the
/// same way.
pub fn offline_env() -> bool {
    std::env::var("RALPHY_RELEASE_OFFLINE")
        .ok()
        .is_some_and(|v| v.trim() == "1")
}

fn cache_is_fresh(path: &Path, ttl: Duration) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) else {
        return false;
    };
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) else {
        return false;
    };
    let age = Utc::now().signed_duration_since(dt.with_timezone(&Utc));
    // A cache stamped in the future is a skewed clock, not a stale file.
    if age < chrono::Duration::zero() {
        return true;
    }
    age.to_std().is_ok_and(|d| d < ttl)
}

fn fetch_releases(url: &str) -> Result<Vec<Release>, String> {
    let body = fetch_body(url)?;
    serde_json::from_str::<Vec<Release>>(&body).map_err(|e| format!("malformed releases JSON: {e}"))
}

fn fetch_body(url: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build();

    let mut last_err = String::from("release fetch failed");
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(RETRY_SLEEP);
        }
        match agent
            .get(url)
            .set("User-Agent", USER_AGENT)
            .set("Accept", "application/vnd.github+json")
            .call()
        {
            Ok(resp) => {
                return resp
                    .into_string()
                    .map_err(|e| format!("reading the releases body: {e}"));
            }
            Err(ureq::Error::Status(code, _)) => {
                last_err = format!("releases HTTP {code}");
                if code == 429 || (500..600).contains(&code) {
                    continue;
                }
                return Err(last_err);
            }
            Err(ureq::Error::Transport(t)) => {
                last_err = format!("releases transport error: {t}");
                continue;
            }
        }
    }
    Err(last_err)
}

/// Write via temp file + atomic rename, so a concurrent reader sees either the
/// old cache or the new one and never a gap. `std::fs::rename` replaces the
/// destination on both Unix and Windows.
fn atomic_write_cache(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("releases.json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::thread;

    fn fixture_body() -> String {
        r#"[
  {"tag_name":"v0.1.0-rc.20","name":"v0.1.0-rc.20","published_at":"2026-09-08T10:00:00Z",
   "html_url":"https://example.invalid/20","prerelease":true,"draft":false,"body":"notes"},
  {"tag_name":"v0.1.0-rc19","name":"v0.1.0-rc19","published_at":"2026-09-07T10:00:00Z",
   "html_url":"https://example.invalid/19","prerelease":true,"draft":false,"body":"notes"}
]"#
        .to_string()
    }

    fn http_response(status: u16, body: &str) -> Vec<u8> {
        let reason = match status {
            200 => "OK",
            403 => "Forbidden",
            429 => "Too Many Requests",
            503 => "Service Unavailable",
            _ => "Error",
        };
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 512];
        loop {
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    /// Bind `127.0.0.1:0` and serve `response` up to `max_accepts` times, or
    /// until the deadline — so a join never hangs when fewer clients come.
    /// Returns the port, the accept counter, and the served request heads.
    #[allow(clippy::type_complexity)]
    fn serve_n(
        response: Vec<u8>,
        max_accepts: u32,
    ) -> (
        u16,
        Arc<AtomicU32>,
        Arc<std::sync::Mutex<Vec<String>>>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("addr").port();
        let accepts = Arc::new(AtomicU32::new(0));
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let accepts_bg = Arc::clone(&accepts);
        let requests_bg = Arc::clone(&requests);
        let handle = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while accepts_bg.load(Ordering::SeqCst) < max_accepts
                && std::time::Instant::now() < deadline
            {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        accepts_bg.fetch_add(1, Ordering::SeqCst);
                        let head = read_request(&mut stream);
                        if let Ok(mut seen) = requests_bg.lock() {
                            seen.push(head);
                        }
                        let _ = stream.write_all(&response);
                        let _ = stream.flush();
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        thread::sleep(Duration::from_millis(20));
        (port, accepts, requests, handle)
    }

    fn temp_cache_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "ralphy-release-test-{}-{tag}/releases.json",
            std::process::id()
        ))
    }

    fn opts<'a>(url: &'a str, cache: &'a Path) -> RefreshOpts<'a> {
        RefreshOpts {
            url,
            cache_path: cache,
            ttl: CACHE_TTL,
            force: false,
            offline: false,
        }
    }

    #[test]
    fn a_missing_cache_is_fetched_and_written() {
        let (port, accepts, requests, handle) = serve_n(http_response(200, &fixture_body()), 1);
        let cache = temp_cache_path("write");
        let _ = std::fs::remove_file(&cache);

        refresh_if_stale(&opts(&format!("http://127.0.0.1:{port}/"), &cache));
        handle.join().expect("server thread");

        assert_eq!(accepts.load(Ordering::SeqCst), 1);
        let seen = requests.lock().expect("requests").clone();
        assert!(
            seen[0].contains("User-Agent: ralphy"),
            "GitHub refuses a request with no user agent; got: {:?}",
            seen[0]
        );
        let releases = load(&cache);
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].tag_name, "v0.1.0-rc.20");
        let _ = std::fs::remove_file(&cache);
    }

    #[test]
    fn a_fresh_cache_is_not_refetched() {
        let (port, accepts, _requests, handle) = serve_n(http_response(200, &fixture_body()), 2);
        let cache = temp_cache_path("fresh");
        let _ = std::fs::remove_file(&cache);
        let url = format!("http://127.0.0.1:{port}/");

        refresh_if_stale(&opts(&url, &cache));
        refresh_if_stale(&opts(&url, &cache));
        drop(handle);

        assert_eq!(
            accepts.load(Ordering::SeqCst),
            1,
            "the second pass must read the cache, not the network"
        );
        let _ = std::fs::remove_file(&cache);
    }

    #[test]
    fn a_failed_fetch_leaves_the_prior_cache_alone() {
        let cache = temp_cache_path("degrade");
        let (ok_port, _a, _r, ok_handle) = serve_n(http_response(200, &fixture_body()), 1);
        let _ = std::fs::remove_file(&cache);
        refresh_if_stale(&opts(&format!("http://127.0.0.1:{ok_port}/"), &cache));
        ok_handle.join().expect("server thread");
        assert_eq!(load(&cache).len(), 2);

        // 503 twice: retried, then given up on — and the cache is untouched.
        let (bad_port, accepts, _r, bad_handle) = serve_n(http_response(503, "nope"), 2);
        let bad_url = format!("http://127.0.0.1:{bad_port}/");
        let mut forced = opts(&bad_url, &cache);
        forced.force = true;
        refresh_if_stale(&forced);
        bad_handle.join().expect("server thread");

        assert_eq!(accepts.load(Ordering::SeqCst), 2, "5xx is retried once");
        assert_eq!(
            load(&cache).len(),
            2,
            "a failed refresh must not empty what we knew"
        );
        let _ = std::fs::remove_file(&cache);
    }

    #[test]
    fn a_malformed_body_does_not_poison_the_cache() {
        let cache = temp_cache_path("malformed");
        let _ = std::fs::remove_file(&cache);
        let (port, _a, _r, handle) = serve_n(http_response(200, "{ not a list }"), 1);
        refresh_if_stale(&opts(&format!("http://127.0.0.1:{port}/"), &cache));
        handle.join().expect("server thread");

        assert!(load(&cache).is_empty());
        assert!(
            !cache.exists(),
            "an unparseable body must not be written at all"
        );
    }

    #[test]
    fn offline_never_opens_a_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("addr").port();
        let cache = temp_cache_path("offline");
        let _ = std::fs::remove_file(&cache);

        let url = format!("http://127.0.0.1:{port}/");
        let mut o = opts(&url, &cache);
        o.offline = true;
        refresh_if_stale(&o);

        match listener.accept() {
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            other => panic!("offline must not connect, got: {other:?}"),
        }
    }

    #[test]
    fn an_absent_cache_reads_as_nothing_known() {
        assert!(load(&temp_cache_path("absent-never-written")).is_empty());
    }

    #[test]
    fn the_default_endpoint_is_the_list_not_latest() {
        // `/releases/latest` answers 404 for this repo: every tag is a
        // pre-release and the API excludes those from `latest`.
        assert!(DEFAULT_RELEASES_URL.contains("/releases?"));
        assert!(!DEFAULT_RELEASES_URL.contains("/releases/latest"));
    }
}
