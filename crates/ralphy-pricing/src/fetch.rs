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
pub const DEFAULT_MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// Cache freshness window (ADR-0034 A6): refetch at most once per day.
pub const CACHE_TTL: Duration = Duration::from_hours(24);

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const READ_TIMEOUT: Duration = Duration::from_secs(1);
/// Hard per-request cap covering connect + TLS + body read. The per-phase
/// timeouts above are a floor (they fail a dead connect fast); this bounds the
/// whole request. With `MAX_ATTEMPTS` and `RETRY_SLEEP` the whole fetch stays
/// within ~3.2s worst case.
const REQUEST_TIMEOUT: Duration = Duration::from_millis(1500);
const MAX_ATTEMPTS: u32 = 2;
/// The largest models.dev body the fetch reads (ADR-0072 D9). Measured
/// 2026-10-01 with curl: `api.json` is 5_282_228 bytes (5.04 MiB); the cap is
/// three times that, rounded up. A larger body fails the fetch and leaves the
/// cache as it was.
pub const MAX_MODELS_DEV_BYTES: u64 = 16 * 1024 * 1024;
const RETRY_SLEEP: Duration = Duration::from_millis(200);

/// Options for a best-effort models.dev refresh. `url` is injectable so tests
/// can point at a loopback listener.
pub struct RefreshOpts<'a> {
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
pub fn refresh_if_stale(opts: &RefreshOpts<'_>) {
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
pub fn pricing_offline_env() -> bool {
    std::env::var("RALPHY_PRICING_OFFLINE").is_ok_and(|v| v.trim() == "1")
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

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(READ_TIMEOUT))
        .timeout_recv_body(Some(READ_TIMEOUT))
        .timeout_global(Some(REQUEST_TIMEOUT))
        // Don't follow redirects: each hop is a fresh connect+read that would
        // blow past the ~3s budget. models.dev/api.json answers 200 directly. With
        // redirects off, ureq returns a 3xx as `Ok` (only >= 400 is
        // `Error::StatusCode`), so its body fails the JSON parse and the caller
        // falls back to the stale cache.
        .max_redirects(0)
        // ureq 3 reads HTTPS_PROXY and friends by default; Ralphy never has
        // (#443).
        .proxy(None)
        .build()
        .into()
}

fn fetch_body(url: &str) -> Result<String, String> {
    let agent = agent();
    let mut last_err = String::from("models.dev fetch failed");
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(RETRY_SLEEP);
        }
        match agent.get(url).call() {
            Ok(mut resp) => {
                return resp
                    .body_mut()
                    .with_config()
                    .limit(MAX_MODELS_DEV_BYTES)
                    .read_to_string()
                    .map_err(|e| format!("reading models.dev body: {e}"));
            }
            Err(ureq::Error::StatusCode(code)) => {
                last_err = format!("models.dev HTTP {code}");
                if code == 429 || (500..600).contains(&code) {
                    continue;
                }
                return Err(last_err);
            }
            Err(e) => {
                last_err = format!("models.dev transport error: {e}");
                continue;
            }
        }
    }
    Err(last_err)
}

/// Write `bytes` via temp file + atomic rename. `std::fs::rename` replaces an
/// existing destination on both Unix and Windows (there it maps to
/// `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`), so the canonical cache is
/// never absent: a concurrent reader sees either the old file or the new one,
/// never a gap. Deleting the destination first would open exactly that gap.
fn atomic_write_cache(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("models-dev.json.{}.tmp", std::process::id()));
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

#[cfg(test)]
mod tests;
