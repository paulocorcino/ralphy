use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;

// The same test harness is in `ralphy-pricing/src/fetch.rs`. Each crate tests
// its own HTTP agent, and the two crates share no code (ADR-0056 §4).
// jscpd:ignore-start
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
// jscpd:ignore-end

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

// The same test harness is in `ralphy-pricing/src/fetch.rs`. Each crate tests
// its own HTTP agent, and the two crates share no code (ADR-0056 §4).
// jscpd:ignore-start
/// Bind `127.0.0.1:0` and serve `response` up to `max_accepts` times, or
/// until the deadline — so a join never hangs when fewer clients come.
/// Returns the port, the accept counter, and the served request heads.
#[expect(
    clippy::type_complexity,
    reason = "one tuple of the port, the counters and the join handle, used by test code only"
)]
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
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "a failure shows in the read that follows"
                    )]
                    let _ = stream.set_nonblocking(false);
                    accepts_bg.fetch_add(1, Ordering::SeqCst);
                    let head = read_request(&mut stream);
                    if let Ok(mut seen) = requests_bg.lock() {
                        seen.push(head);
                    }
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "the test client may have closed its end before the reply"
                    )]
                    let _ = stream.write_all(&response);
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "the test client may have closed its end before the reply"
                    )]
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
// jscpd:ignore-end

fn temp_cache_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "ralphy-release-test-{}-{tag}/releases.json",
        std::process::id()
    ))
}

fn remove_cache(cache: &Path) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "cleanup of a test file that may not exist, and no test reads it after"
    )]
    let _ = std::fs::remove_file(cache);
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
    remove_cache(&cache);

    refresh_if_stale(&opts(&format!("http://127.0.0.1:{port}/"), &cache));
    handle.join().expect("server thread");

    assert_eq!(accepts.load(Ordering::SeqCst), 1);
    let seen = requests.lock().expect("requests").clone();
    // Header names are case-insensitive (ureq 3 sends them lowercase), so
    // they are compared that way; values are compared exactly.
    let headers: Vec<(String, &str)> = seen[0]
        .split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .collect();
    // The exact value, not a prefix: `ralphy/0.1.0 (windows; x86_64)` is
    // precisely the identifier ADR-0056 §9 says this request does not carry.
    let agents: Vec<&str> = headers
        .iter()
        .filter(|(name, _)| name == "user-agent")
        .map(|(_, value)| *value)
        .collect();
    assert_eq!(
        agents,
        ["ralphy"],
        "the agent names the product and nothing else; got: {:?}",
        seen[0]
    );
    for (name, _) in &headers {
        assert!(
            name != "cookie" && name != "authorization" && !name.starts_with("x-"),
            "the request must carry no {name} header; got: {:?}",
            seen[0]
        );
    }
    let releases = load(&cache);
    assert_eq!(releases.len(), 2);
    assert_eq!(releases[0].tag_name, "v0.1.0-rc.20");
    remove_cache(&cache);
}

#[test]
fn a_fresh_cache_is_not_refetched() {
    let (port, accepts, _requests, handle) = serve_n(http_response(200, &fixture_body()), 2);
    let cache = temp_cache_path("fresh");
    remove_cache(&cache);
    let url = format!("http://127.0.0.1:{port}/");

    refresh_if_stale(&opts(&url, &cache));
    refresh_if_stale(&opts(&url, &cache));
    drop(handle);

    assert_eq!(
        accepts.load(Ordering::SeqCst),
        1,
        "the second pass must read the cache, not the network"
    );
    remove_cache(&cache);
}

#[test]
fn a_stale_cache_is_refetched() {
    // The gap this closes: every other case here is missing, fresh, or
    // force-bypassed, so a `cache_is_fresh` that answered true for any
    // existing file passed the whole suite — and the daemon would then
    // never refresh after its first successful fetch.
    let (port, accepts, _r, handle) = serve_n(http_response(200, &fixture_body()), 2);
    let cache = temp_cache_path("stale");
    remove_cache(&cache);
    let url = format!("http://127.0.0.1:{port}/");

    let mut o = opts(&url, &cache);
    o.ttl = Duration::ZERO;
    refresh_if_stale(&o);
    refresh_if_stale(&o);
    drop(handle);

    assert_eq!(
        accepts.load(Ordering::SeqCst),
        2,
        "a cache older than its ttl must be refetched, not served"
    );
    remove_cache(&cache);
}

#[test]
fn a_cache_stamped_in_the_future_is_a_skewed_clock_not_a_stale_file() {
    let cache = temp_cache_path("skew");
    remove_cache(&cache);
    std::fs::create_dir_all(cache.parent().expect("parent")).expect("dir");
    std::fs::write(
        &cache,
        r#"{"timestamp":"2099-01-01T00:00:00Z","releases":[]}"#,
    )
    .expect("write");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let port = listener.local_addr().expect("addr").port();
    let url = format!("http://127.0.0.1:{port}/");
    refresh_if_stale(&opts(&url, &cache));

    match listener.accept() {
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut => {}
        other => panic!("a future timestamp must read as fresh, got: {other:?}"),
    }
    remove_cache(&cache);
}

#[test]
fn only_a_retryable_status_is_retried() {
    // The retry policy is "429 and 5xx, nothing else" (5xx: see
    // `a_failed_fetch_leaves_the_prior_cache_alone`); without this a
    // version that retried everything, or nothing, passed.
    for (status, want, why) in [
        (403, 1, "403 is given up on at once"),
        (429, 2, "429 is retried once"),
    ] {
        let (port, accepts, _r, handle) = serve_n(http_response(status, "no"), 2);
        let cache = temp_cache_path(&format!("status-{status}"));
        remove_cache(&cache);
        refresh_if_stale(&opts(&format!("http://127.0.0.1:{port}/"), &cache));
        drop(handle);
        assert_eq!(accepts.load(Ordering::SeqCst), want, "{why}");
        remove_cache(&cache);
    }
}

#[test]
fn a_failed_fetch_leaves_the_prior_cache_alone() {
    let cache = temp_cache_path("degrade");
    let (ok_port, _a, _r, ok_handle) = serve_n(http_response(200, &fixture_body()), 1);
    remove_cache(&cache);
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
    remove_cache(&cache);
}

#[test]
fn a_malformed_body_does_not_poison_the_cache() {
    let cache = temp_cache_path("malformed");
    remove_cache(&cache);
    let (port, _a, _r, handle) = serve_n(http_response(200, "{ not a list }"), 1);
    refresh_if_stale(&opts(&format!("http://127.0.0.1:{port}/"), &cache));
    handle.join().expect("server thread");

    assert_eq!(load(&cache), Vec::<Release>::new());
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
    remove_cache(&cache);

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
fn a_release_is_read_by_its_tag() {
    let body = concat!(
        r#"{"tag_name":"v0.1.0-rc.7","name":"v0.1.0-rc.7","#,
        r#""published_at":"2026-08-01T10:00:00Z",
   "html_url":"https://example.invalid/7","prerelease":true,"draft":false,"body":"notes",
   "assets":[{"name":"ralphy-v0.1.0-rc.7-linux-x64.tar.gz","#,
        r#""browser_download_url":"https://example.invalid/a","size":3}]}"#
    );
    let (port, _a, requests, handle) = serve_n(http_response(200, body), 1);
    let release = fetch_release_by_tag(&format!("http://127.0.0.1:{port}/tags/"), "v0.1.0-rc.7")
        .expect("the release");
    handle.join().expect("server thread");
    let head = requests.lock().expect("requests")[0].clone();
    assert!(head.starts_with("GET /tags/v0.1.0-rc.7 "), "{head:?}");
    assert_eq!(release.tag_name, "v0.1.0-rc.7");
    assert_eq!(release.assets.len(), 1);
}

#[test]
fn an_absent_cache_reads_as_nothing_known() {
    assert_eq!(
        load(&temp_cache_path("absent-never-written")),
        Vec::<Release>::new()
    );
}
