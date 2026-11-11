use super::*;
use crate::tests::{one_million_each, remove_scratch, ENV_LOCK};
use crate::PriceTable;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// ureq 3 reads a proxy from the environment when an agent is built; Ralphy
/// never has, and #443 keeps it that way. The default agent proves the
/// variable is one ureq reads, so the check on ours is not empty.
#[test]
fn the_agent_ignores_a_proxy_in_the_environment() {
    let saved = std::env::var_os("ALL_PROXY");
    std::env::set_var("ALL_PROXY", "http://127.0.0.1:9");
    let default_uses_it = ureq::Agent::new_with_defaults().config().proxy().is_some();
    let ours_uses_it = agent().config().proxy().is_some();
    match saved {
        Some(v) => std::env::set_var("ALL_PROXY", v),
        None => std::env::remove_var("ALL_PROXY"),
    }
    assert!(default_uses_it, "ureq no longer reads ALL_PROXY");
    assert!(!ours_uses_it, "the agent picked up ALL_PROXY");
}

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
    .to_owned()
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
        .to_owned()
}

/// Bind `127.0.0.1:0` and serve `response` up to `max_accepts` times, or
/// until `serve_for` elapses — so a join never hangs when fewer clients come.
fn serve_n(response: Vec<u8>, max_accepts: u32) -> (u16, Arc<AtomicU32>, thread::JoinHandle<()>) {
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
                    // A failure shows in the read that follows.
                    if let Err(e) = stream.set_nonblocking(false) {
                        tracing::debug!("set_nonblocking failed: {e}");
                    }
                    accepts_bg.fetch_add(1, Ordering::SeqCst);
                    let line = read_request_line(&mut stream);
                    assert!(line.starts_with("GET "), "expected GET, got: {line:?}");
                    // The test client may have closed its end before the reply.
                    if let Err(e) = stream.write_all(&response) {
                        tracing::debug!("reply not written: {e}");
                    }
                    if let Err(e) = stream.flush() {
                        tracing::debug!("reply not flushed: {e}");
                    }
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
        concat!(
            r#"{{"timestamp":"{timestamp}","data":{{"anthropic/claude-opus-4-8":"#,
            r#"{{"input":{opus_input},"output":25.0,"cache_read":0.5,"cache_creation":6.25}}}}}}"#
        ),
        timestamp = timestamp,
        opus_input = opus_input
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
    handle.join().expect("server thread");

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

    remove_scratch(cache.parent().unwrap());
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
    handle.join().expect("server thread");
    assert_eq!(
        accepts.load(Ordering::SeqCst),
        1,
        "force must GET despite fresh cache"
    );

    remove_scratch(cache.parent().unwrap());
}

#[test]
fn stale_ttl_triggers_one_get() {
    let body = fixture_body();
    let (port, accepts, handle) = serve_n(http_response(200, &body), 1);
    let url = format!("http://127.0.0.1:{port}/api.json");
    let cache = temp_cache_path("ttl-stale");
    let old = (Utc::now() - chrono::Duration::hours(25)).to_rfc3339_opts(SecondsFormat::Secs, true);
    write_cache(&cache, &old, 5.0);

    refresh_if_stale(&opts(&url, &cache, false, false));
    handle.join().expect("server thread");
    assert_eq!(
        accepts.load(Ordering::SeqCst),
        1,
        "timestamp ≥25h ago must GET"
    );

    remove_scratch(cache.parent().unwrap());
}

/// A fetch that fails falls back: after its retries it creates no cache, and
/// it leaves a prior cache byte for byte.
#[test]
fn a_failed_fetch_falls_back_and_leaves_the_cache_alone() {
    let prior = concat!(
        r#"{"timestamp":"2020-01-01T00:00:00Z","data":{"anthropic/claude-opus-4-8":"#,
        r#"{"input":5.0,"output":25.0,"cache_read":0.5,"cache_creation":6.25}}}"#
    )
    .as_bytes();
    // (case, served status and body; `None` means nothing listens)
    let rows: [(&str, Option<(u16, &str)>); 3] = [
        ("http-503", Some((503, "nope"))),
        ("http-429", Some((429, "slow down"))),
        ("transport-error", None),
    ];
    for (case, served) in rows {
        let cache = temp_cache_path(case);
        let server = served.map(|(status, body)| serve_n(http_response(status, body), 4));
        let url = match &server {
            Some((port, ..)) => format!("http://127.0.0.1:{port}/api.json"),
            None => "http://127.0.0.1:1/api.json".to_owned(),
        };

        refresh_if_stale(&opts(&url, &cache, false, false));
        if let Some((_, accepts, _)) = &server {
            assert!(
                accepts.load(Ordering::SeqCst) >= 2,
                "{case}: must retry (2 attempts); got {}",
                accepts.load(Ordering::SeqCst)
            );
        }
        assert!(
            !cache.exists(),
            "{case}: failed fetch must not create cache"
        );

        std::fs::write(&cache, prior).expect("prior");
        refresh_if_stale(&opts(&url, &cache, true, false));
        if let Some((_, _, handle)) = server {
            handle.join().expect("server thread");
        }
        let after = std::fs::read(&cache).unwrap();
        assert_eq!(after, prior, "{case}: must not rewrite prior cache");

        remove_scratch(cache.parent().unwrap());
    }
}

/// A body over the cap fails the fetch, is not retried, and leaves the
/// prior cache bytes as they were.
#[test]
fn an_oversized_body_fails_and_leaves_the_cache() {
    let filler = "a".repeat(usize::try_from(MAX_MODELS_DEV_BYTES).expect("16 MiB fits usize"));
    let body = format!("{{\"x\":\"{filler}\"}}");
    assert!(u64::try_from(body.len()).expect("a length fits u64") > MAX_MODELS_DEV_BYTES);
    let (port, accepts, handle) = serve_n(http_response(200, &body), 1);
    let url = format!("http://127.0.0.1:{port}/api.json");
    assert!(fetch_body(&url).is_err(), "an oversized body was read");
    handle.join().expect("server thread");
    assert_eq!(
        accepts.load(Ordering::SeqCst),
        1,
        "no retry on an oversized body"
    );

    let cache = temp_cache_path("oversized");
    let prior = br#"{"timestamp":"2020-01-01T00:00:00Z","data":{}}"#;
    std::fs::write(&cache, prior).expect("prior");
    let (port, _accepts, handle) = serve_n(http_response(200, &body), 1);
    let url = format!("http://127.0.0.1:{port}/api.json");
    refresh_if_stale(&opts(&url, &cache, true, false));
    handle.join().expect("server thread");
    assert_eq!(std::fs::read(&cache).unwrap(), prior);
    remove_scratch(cache.parent().unwrap());
}

#[test]
fn malformed_json_leaves_prior_cache_bytes_unchanged() {
    let (port, accepts, handle) = serve_n(http_response(200, "not-json"), 1);
    let url = format!("http://127.0.0.1:{port}/api.json");
    let cache = temp_cache_path("malformed");
    let prior = concat!(
        r#"{"timestamp":"2020-01-01T00:00:00Z","data":{"anthropic/claude-opus-4-8":"#,
        r#"{"input":5.0,"output":25.0,"cache_read":0.5,"cache_creation":6.25}}}"#
    )
    .as_bytes();
    std::fs::write(&cache, prior).expect("prior");
    let before = std::fs::read(&cache).unwrap();

    refresh_if_stale(&opts(&url, &cache, true, false));
    handle.join().expect("server thread");
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
    // The prior cache still prices: 5 + 25 + 0.5 + 6.25 over 1M of each.
    assert_eq!(
        table.cost_usd("claude-opus-4-8", &one_million_each()),
        Some(36.75)
    );
    assert!(table
        .cost_usd("not-a-real-model-zz", &one_million_each())
        .is_none());
    std::env::remove_var("RALPHY_PRICING_CACHE");
    std::env::remove_var("RALPHY_PRICING_FILE");

    remove_scratch(cache.parent().unwrap());
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

    remove_scratch(cache.parent().unwrap());
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

    let offline = crate::pricing_offline_from_file();
    assert!(offline, "toml offline = true must be detected");
    refresh_if_stale(&opts(&url, &cache, true, offline));
    assert_no_accept(&listener);
    std::env::remove_var("RALPHY_PRICING_FILE");

    remove_scratch(cache.parent().unwrap());
}

#[test]
fn manifest_excludes_core_and_agent_crates() {
    // The dependency names, from every dependency table of the manifest:
    // text in a comment or a description is not a dependency.
    let manifest: toml::Table =
        toml::from_str(include_str!("../../Cargo.toml")).expect("Cargo.toml parses");
    let tables = ["dependencies", "dev-dependencies", "build-dependencies"];
    let targets = manifest
        .get("target")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(|t| t.values().filter_map(toml::Value::as_table));
    let deps: Vec<&str> = std::iter::once(&manifest)
        .chain(targets)
        .flat_map(|t| tables.iter().filter_map(|k| t.get(*k)))
        .filter_map(toml::Value::as_table)
        .flat_map(|t| t.keys().map(String::as_str))
        .collect();
    assert!(
        deps.contains(&"ureq"),
        "ralphy-pricing must depend on ureq: {deps:?}"
    );
    // ADR-0032 §10: the daemon must be able to depend on this crate without
    // dragging in core or a vendor adapter — including via dev-dependencies.
    // That is what sent the two adapter-keyed floor tests to the CLI.
    for dep in &deps {
        for banned in ["reqwest", "tokio", "ralphy-core", "ralphy-agent"] {
            assert!(
                !dep.starts_with(banned),
                "ralphy-pricing must not depend on {dep}"
            );
        }
    }
}
