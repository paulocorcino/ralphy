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
/// Whole-request cap covering connect, TLS and body read. Generous compared to
/// the pricing fetch because nothing waits on this one — it is a background
/// poll, never a step in a run.
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

// The same code is in `ralphy-pricing/src/fetch.rs`, and it stays two copies:
// pricing and release are leaf crates with no shared crate (ADR-0056 §4), and
// each owns its outside product with its own timeouts and limits. A fix here
// is checked against the pricing copy by hand.
// jscpd:ignore-start
/// True when `RALPHY_RELEASE_OFFLINE` trims to `"1"`. Mirrors the pricing
/// crate's switch so an air-gapped operator turns off every outbound read the
/// same way.
pub fn offline_env() -> bool {
    std::env::var("RALPHY_RELEASE_OFFLINE").is_ok_and(|v| v.trim() == "1")
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
// jscpd:ignore-end

/// The prefix of one release's URL; the tag follows it.
pub const RELEASE_BY_TAG_URL: &str =
    "https://api.github.com/repos/paulocorcino/ralphy/releases/tags/";

/// One release by its tag, read now with no cache. The cached list holds only
/// the newest ten releases, so an older tag must be asked for by name. `base`
/// is [`RELEASE_BY_TAG_URL`], or a loopback URL in tests.
pub fn fetch_release_by_tag(base: &str, tag: &str) -> Result<Release, String> {
    let body = fetch_body(&format!("{base}{tag}"))?;
    serde_json::from_str::<Release>(&body).map_err(|e| format!("malformed release JSON: {e}"))
}

fn fetch_releases(url: &str) -> Result<Vec<Release>, String> {
    let body = fetch_body(url)?;
    serde_json::from_str::<Vec<Release>>(&body).map_err(|e| format!("malformed releases JSON: {e}"))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(READ_TIMEOUT))
        .timeout_recv_body(Some(READ_TIMEOUT))
        .timeout_global(Some(REQUEST_TIMEOUT))
        // ureq 3 reads HTTPS_PROXY and friends by default; Ralphy never has
        // (#443).
        .proxy(None)
        .build()
        .into()
}

fn fetch_body(url: &str) -> Result<String, String> {
    let agent = agent();
    let mut last_err = String::from("release fetch failed");
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(RETRY_SLEEP);
        }
        match agent
            .get(url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/vnd.github+json")
            .call()
        {
            Ok(mut resp) => {
                return resp
                    .body_mut()
                    .read_to_string()
                    .map_err(|e| format!("reading the releases body: {e}"));
            }
            Err(ureq::Error::StatusCode(code)) => {
                last_err = format!("releases HTTP {code}");
                if code == 429 || (500..600).contains(&code) {
                    continue;
                }
                return Err(last_err);
            }
            Err(e) => {
                last_err = format!("releases transport error: {e}");
                continue;
            }
        }
    }
    Err(last_err)
}

// The same code is in `ralphy-pricing/src/fetch.rs`, and it stays two copies:
// pricing and release are leaf crates with no shared crate (ADR-0056 §4), and
// each owns its outside product with its own timeouts and limits. A fix here
// is checked against the pricing copy by hand.
// jscpd:ignore-start
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
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the rename error is the one to return; a failed cleanup would mask it"
            )]
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
// jscpd:ignore-end

#[cfg(test)]
mod tests;
