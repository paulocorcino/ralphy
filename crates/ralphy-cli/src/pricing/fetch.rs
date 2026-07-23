//! models.dev fetch + TTL disk-cache write (ADR-0034 A5/A6). Triggered only by
//! `ralphy usage` via [`refresh_if_stale`]; run/footer paths never call here.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use tracing::warn;

use super::ingest::ingest_models_dev;
use super::ModelPrice;

/// Official models.dev catalog endpoint (README).
pub(crate) const DEFAULT_MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// Cache freshness window (ADR-0034 A6): refetch at most once per day.
pub(crate) const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const READ_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_ATTEMPTS: u32 = 2;
const RETRY_SLEEP: Duration = Duration::from_millis(200);

/// Options for a best-effort models.dev refresh. `url` is injectable so tests
/// can point at a loopback listener.
pub(crate) struct RefreshOpts<'a> {
    pub url: &'a str,
    pub cache_path: &'a Path,
    pub ttl: Duration,
    pub force: bool,
    pub offline: bool,
}

#[derive(Serialize)]
struct CacheEnvelope {
    timestamp: String,
    data: BTreeMap<String, ModelPrice>,
}

/// When the cache is missing/stale (or `force`), GET `opts.url`, ingest, and
/// atomically rewrite the cache. Offline, fresh (and not forced), or any fetch
/// failure leaves the prior cache alone and returns without error — callers
/// always fall through to [`super::PriceTable::load`].
pub(crate) fn refresh_if_stale(opts: &RefreshOpts<'_>) {
    if opts.offline {
        return;
    }
    if !opts.force && cache_is_fresh(opts.cache_path, opts.ttl) {
        return;
    }
    match fetch_and_ingest(opts.url) {
        Ok(data) => {
            let envelope = CacheEnvelope {
                timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                data,
            };
            match serde_json::to_vec_pretty(&envelope) {
                Ok(bytes) => {
                    if let Err(e) = atomic_write_cache(opts.cache_path, &bytes) {
                        warn!(
                            path = %opts.cache_path.display(),
                            error = %e,
                            "writing pricing cache failed — keeping prior cache/seed"
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "serializing pricing cache failed — keeping prior cache/seed"
                    );
                }
            }
        }
        Err(e) => {
            warn!(
                error = %e,
                "models.dev pricing fetch failed — using stale cache or seed"
            );
        }
    }
}

/// True when `RALPHY_PRICING_OFFLINE` trims to `"1"`.
pub(crate) fn pricing_offline_env() -> bool {
    std::env::var("RALPHY_PRICING_OFFLINE")
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
    if age < chrono::Duration::zero() {
        return true;
    }
    age.to_std().is_ok_and(|d| d < ttl)
}

fn fetch_and_ingest(url: &str) -> Result<BTreeMap<String, ModelPrice>, String> {
    let body = fetch_body(url)?;
    let doc: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("malformed models.dev JSON: {e}"))?;
    Ok(ingest_models_dev(&doc))
}

fn fetch_body(url: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build();

    let mut last_err = String::from("models.dev fetch failed");
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(RETRY_SLEEP);
        }
        match agent.get(url).call() {
            Ok(resp) => {
                return resp
                    .into_string()
                    .map_err(|e| format!("reading models.dev body: {e}"));
            }
            Err(ureq::Error::Status(code, _)) => {
                last_err = format!("models.dev HTTP {code}");
                if code == 429 || (500..600).contains(&code) {
                    continue;
                }
                return Err(last_err);
            }
            Err(ureq::Error::Transport(t)) => {
                last_err = format!("models.dev transport error: {t}");
                continue;
            }
        }
    }
    Err(last_err)
}

/// Write `bytes` via temp file + rename. On Windows, remove the destination
/// first — `rename` does not replace an existing file.
fn atomic_write_cache(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("models-dev.json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    #[cfg(windows)]
    {
        let _ = std::fs::remove_file(path);
    }
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
    use crate::pricing::tests::{one_million_each, ENV_LOCK};
    use crate::pricing::PriceTable;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    /// Minimal models.dev-shaped fixture: opus at input 9.0 (≠ seed 15.0) so a
    /// no-op pass is impossible, plus a `$0` row that ingest must drop.
    fn fixture_body() -> String {
        r#"{
  "anthropic": {
    "models": {
      "claude-opus-4-8": {
        "cost": { "input": 9.0, "output": 45.0, "cache_read": 0.9, "cache_write": 11.25 }
      },
      "free-model": {
        "cost": { "input": 0, "output": 0 }
      }
    }
  }
}"#
        .to_string()
    }

    fn http_response(status: u16, body: &str) -> Vec<u8> {
        let reason = match status {
            200 => "OK",
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

    fn read_request_line(stream: &mut std::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 512];
        loop {
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
            let n = match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            buf.extend_from_slice(&chunk[..n]);
        }
        String::from_utf8_lossy(&buf)
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// Bind `127.0.0.1:0` and serve `response` up to `max_accepts` times, or
    /// until `serve_for` elapses — so a join never hangs when fewer clients come.
    fn serve_n(
        response: Vec<u8>,
        max_accepts: u32,
    ) -> (u16, Arc<AtomicU32>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().unwrap().port();
        let accepts = Arc::new(AtomicU32::new(0));
        let accepts_bg = Arc::clone(&accepts);
        let handle = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while accepts_bg.load(Ordering::SeqCst) < max_accepts
                && std::time::Instant::now() < deadline
            {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Blocking reads for the request body are fine once connected.
                        let _ = stream.set_nonblocking(false);
                        accepts_bg.fetch_add(1, Ordering::SeqCst);
                        let line = read_request_line(&mut stream);
                        assert!(line.starts_with("GET "), "expected GET, got: {line:?}");
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
        (port, accepts, handle)
    }

    /// Live listener that never enters `accept` — used to prove offline skips.
    fn live_listener() -> (u16, TcpListener) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().unwrap().port();
        (port, listener)
    }

    fn assert_no_accept(listener: &TcpListener) {
        match listener.accept() {
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Ok(_) => panic!("offline/fresh path must not connect"),
            Err(e) => panic!("unexpected accept error: {e}"),
        }
    }

    fn temp_cache_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ralphy-pricing-fetch-{}-{}-{}",
            tag,
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join("models-dev.json")
    }

    fn write_cache(path: &Path, timestamp: &str, opus_input: f64) {
        let body = format!(
            r#"{{"timestamp":"{timestamp}","data":{{"anthropic/claude-opus-4-8":{{"input":{opus_input},"output":25.0,"cache_read":0.5,"cache_creation":6.25}}}}}}"#
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("cache dir");
        }
        std::fs::write(path, body).expect("write cache");
    }

    fn opts<'a>(url: &'a str, cache_path: &'a Path, force: bool, offline: bool) -> RefreshOpts<'a> {
        RefreshOpts {
            url,
            cache_path,
            ttl: CACHE_TTL,
            force,
            offline,
        }
    }

    #[test]
    fn stale_cache_fetches_and_writes_normalized_data() {
        let body = fixture_body();
        let (port, accepts, handle) = serve_n(http_response(200, &body), 1);
        let url = format!("http://127.0.0.1:{port}/api.json");
        let cache = temp_cache_path("stale");
        // Missing cache ⇒ stale.
        refresh_if_stale(&opts(&url, &cache, false, false));
        handle.join().ok();

        assert_eq!(accepts.load(Ordering::SeqCst), 1, "exactly one GET");
        let written = std::fs::read_to_string(&cache).expect("cache written");
        let v: serde_json::Value = serde_json::from_str(&written).expect("cache json");
        let input = v["data"]["anthropic/claude-opus-4-8"]["input"]
            .as_f64()
            .expect("opus input");
        assert_eq!(
            input, 9.0,
            "fetched rate must be fixture 9.0, not seed 15.0"
        );
        assert!(
            v["data"].get("anthropic/free-model").is_none(),
            "$0 free-model must not appear in written cache"
        );
        assert!(
            v["data"].get("free-model").is_none(),
            "$0 bare key must not appear either"
        );

        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }

    #[test]
    fn fresh_ttl_skips_fetch_force_refetches() {
        let cache = temp_cache_path("fresh");
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        write_cache(&cache, &now, 5.0);

        // Fresh TTL: live listener must see zero accepts.
        let (port, listener) = live_listener();
        let url = format!("http://127.0.0.1:{port}/api.json");
        refresh_if_stale(&opts(&url, &cache, false, false));
        assert_no_accept(&listener);
        drop(listener);

        // Force: one GET despite fresh timestamp.
        let body = fixture_body();
        let (port, accepts, handle) = serve_n(http_response(200, &body), 1);
        let url = format!("http://127.0.0.1:{port}/api.json");
        refresh_if_stale(&opts(&url, &cache, true, false));
        handle.join().ok();
        assert_eq!(
            accepts.load(Ordering::SeqCst),
            1,
            "force must GET despite fresh cache"
        );

        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }

    #[test]
    fn stale_ttl_triggers_one_get() {
        let body = fixture_body();
        let (port, accepts, handle) = serve_n(http_response(200, &body), 1);
        let url = format!("http://127.0.0.1:{port}/api.json");
        let cache = temp_cache_path("ttl-stale");
        let old =
            (Utc::now() - chrono::Duration::hours(25)).to_rfc3339_opts(SecondsFormat::Secs, true);
        write_cache(&cache, &old, 5.0);

        refresh_if_stale(&opts(&url, &cache, false, false));
        handle.join().ok();
        assert_eq!(
            accepts.load(Ordering::SeqCst),
            1,
            "timestamp ≥25h ago must GET"
        );

        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }

    #[test]
    fn http_503_after_retries_leaves_no_or_prior_cache() {
        let (port, accepts, handle) = serve_n(http_response(503, "nope"), 4);
        let url = format!("http://127.0.0.1:{port}/api.json");
        let cache = temp_cache_path("503");
        assert!(!cache.exists());

        refresh_if_stale(&opts(&url, &cache, false, false));
        assert!(
            accepts.load(Ordering::SeqCst) >= 2,
            "503 must retry (2 attempts); got {}",
            accepts.load(Ordering::SeqCst)
        );
        assert!(!cache.exists(), "failed fetch must not create cache");

        let prior = br#"{"timestamp":"2020-01-01T00:00:00Z","data":{"anthropic/claude-opus-4-8":{"input":5.0,"output":25.0,"cache_read":0.5,"cache_creation":6.25}}}"#;
        std::fs::write(&cache, prior).expect("prior");
        let before = std::fs::read(&cache).unwrap();
        refresh_if_stale(&opts(&url, &cache, true, false));
        handle.join().ok();
        let after = std::fs::read(&cache).unwrap();
        assert_eq!(before, after, "503 must not rewrite prior cache");

        // Known model still prices from seed (load without cache env).
        let table = PriceTable::defaults();
        assert!(table
            .cost_usd("claude-opus-4-8", &one_million_each())
            .is_some());
        assert!(table
            .cost_usd("not-a-real-model-zz", &one_million_each())
            .is_none());

        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }

    #[test]
    fn http_429_after_retries_falls_back() {
        let (port, accepts, handle) = serve_n(http_response(429, "slow down"), 4);
        let url = format!("http://127.0.0.1:{port}/api.json");
        let cache = temp_cache_path("429");

        refresh_if_stale(&opts(&url, &cache, false, false));
        handle.join().ok();
        assert!(
            accepts.load(Ordering::SeqCst) >= 2,
            "429 must retry; got {}",
            accepts.load(Ordering::SeqCst)
        );
        assert!(!cache.exists());

        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }

    #[test]
    fn transport_error_falls_back() {
        // Nothing listening on this port.
        let url = "http://127.0.0.1:1/api.json";
        let cache = temp_cache_path("transport");
        refresh_if_stale(&opts(url, &cache, false, false));
        assert!(!cache.exists());
        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }

    #[test]
    fn malformed_json_leaves_prior_cache_bytes_unchanged() {
        let (port, accepts, handle) = serve_n(http_response(200, "not-json"), 1);
        let url = format!("http://127.0.0.1:{port}/api.json");
        let cache = temp_cache_path("malformed");
        let prior = br#"{"timestamp":"2020-01-01T00:00:00Z","data":{"anthropic/claude-opus-4-8":{"input":5.0,"output":25.0,"cache_read":0.5,"cache_creation":6.25}}}"#;
        std::fs::write(&cache, prior).expect("prior");
        let before = std::fs::read(&cache).unwrap();

        refresh_if_stale(&opts(&url, &cache, true, false));
        handle.join().ok();
        assert_eq!(accepts.load(Ordering::SeqCst), 1, "no retry on bad JSON");
        let after = std::fs::read(&cache).unwrap();
        assert_eq!(before, after, "malformed body must not poison cache");

        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("RALPHY_PRICING_CACHE", &cache);
        std::env::set_var(
            "RALPHY_PRICING_FILE",
            cache.with_file_name("missing-pricing.toml"),
        );
        let table = PriceTable::load();
        assert!(table
            .cost_usd("claude-opus-4-8", &one_million_each())
            .is_some());
        assert!(table
            .cost_usd("not-a-real-model-zz", &one_million_each())
            .is_none());
        std::env::remove_var("RALPHY_PRICING_CACHE");
        std::env::remove_var("RALPHY_PRICING_FILE");

        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }

    #[test]
    fn offline_env_skips_even_with_force_and_stale() {
        let _g = ENV_LOCK.lock().unwrap();
        let (port, listener) = live_listener();
        let url = format!("http://127.0.0.1:{port}/api.json");
        let cache = temp_cache_path("offline-env");

        std::env::set_var("RALPHY_PRICING_OFFLINE", "1");
        assert!(pricing_offline_env());
        refresh_if_stale(&opts(&url, &cache, true, true));
        assert_no_accept(&listener);
        assert!(!cache.exists());
        std::env::remove_var("RALPHY_PRICING_OFFLINE");

        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }

    #[test]
    fn offline_toml_skips_fetch() {
        let _g = ENV_LOCK.lock().unwrap();
        let (port, listener) = live_listener();
        let url = format!("http://127.0.0.1:{port}/api.json");
        let cache = temp_cache_path("offline-toml");
        let pricing = cache.with_file_name("pricing.toml");
        std::fs::write(&pricing, "offline = true\n").expect("write pricing.toml");
        std::env::set_var("RALPHY_PRICING_FILE", &pricing);

        let offline = crate::pricing::pricing_offline_from_file();
        assert!(offline, "toml offline = true must be detected");
        refresh_if_stale(&opts(&url, &cache, true, offline));
        assert_no_accept(&listener);
        std::env::remove_var("RALPHY_PRICING_FILE");

        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }

    #[test]
    fn cargo_toml_pins_ureq_excludes_reqwest_tokio() {
        let manifest = include_str!("../../Cargo.toml");
        assert!(manifest.contains("ureq"), "ralphy-cli must depend on ureq");
        // Build needles from parts so this file cannot trip an absence pin on itself.
        let reqwest = ["req", "west"].concat();
        let tokio = ["tok", "io"].concat();
        assert!(
            !manifest.contains(&reqwest),
            "ralphy-cli must not depend on {reqwest}"
        );
        assert!(
            !manifest.contains(&tokio),
            "ralphy-cli must not depend on {tokio}"
        );
    }

    #[test]
    fn refresh_if_stale_sole_production_call_is_usage_cmd() {
        // Concatenate so include_str of this file cannot match the needle via its
        // own source text describing the pin.
        let name = ["refresh_if_", "stale"].concat();
        let usage = include_str!("../usage.rs");
        let report = include_str!("../run/report.rs");
        let presenter = include_str!("../ui/presenter.rs");
        let pricing_root = include_str!("../pricing.rs");
        let floor = include_str!("floor.rs");
        let ingest = include_str!("ingest.rs");

        let usage_hits = usage.matches(&name).count();
        assert!(usage_hits >= 1, "usage.rs must call {name}");
        assert_eq!(
            report.matches(&name).count(),
            0,
            "run/report.rs must not call {name}"
        );
        assert_eq!(
            presenter.matches(&name).count(),
            0,
            "ui/presenter.rs must not call {name}"
        );
        assert_eq!(
            pricing_root.matches(&name).count(),
            0,
            "pricing.rs must not call {name}"
        );
        assert_eq!(floor.matches(&name).count(), 0);
        assert_eq!(ingest.matches(&name).count(), 0);
    }
}
