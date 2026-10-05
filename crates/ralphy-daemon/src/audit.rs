//! The audit log (ADR-0074): one JSON line per event in `daemon-audit.jsonl`,
//! owner-only, append-only, pruned to 90 days and 20 MiB. Facts keep their
//! source (D2): `server` is what the daemon read from the request headers.
//!
//! A router whose store has no directory (a test router rooted at a bare
//! relative name) keeps no log and issues no device cookie, so a test never
//! writes into the process's working directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use axum::http::HeaderMap;
use serde::Serialize;

use crate::device::{self, DeviceId, DeviceKey};
use crate::owner_only;

/// Lines older than this are removed (D3).
pub const RETENTION_SECS: i64 = 90 * 24 * 3600;
/// The file is kept under this size (D3).
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;
/// A prune for size cuts down to this, so the next append does not prune again.
const PRUNE_TO_BYTES: u64 = 16 * 1024 * 1024;
/// The longest header value a line keeps.
const MAX_HEADER_CHARS: usize = 512;

/// The `daemon-audit.jsonl` path inside `dir`.
pub fn log_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon-audit.jsonl")
}

/// What an event records (D4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    LoginOk,
    LoginFailed,
    Logout,
    DeviceFacts,
    DeviceProfileChanged,
    Action,
    /// A command-socket verb that changes state (D12).
    Command,
}

/// Who sent the request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    /// A browser with a device cookie.
    Device,
    /// A caller with an `Authorization` header: a peer daemon or a machine
    /// client. The daemon cannot tell which peer sent it (D8).
    Bearer,
    /// Neither, such as a login attempt from a browser with no device cookie.
    Unknown,
}

/// Why a login failed, as far as the response tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginFailure {
    /// A wrong or replayed code, or a wrong password: the route answers both
    /// the same way.
    BadCredential,
    Throttled,
}

/// The request facts the daemon read itself (source `server`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ServerFacts {
    /// The client's public address, as the front reported it in `X-Real-IP`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_for: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_proto: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accept_language: Option<String>,
    /// Every `Sec-CH-UA*` header, by name without the prefix.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub client_hints: Vec<(String, String)>,
}

impl ServerFacts {
    pub fn from_headers(headers: &HeaderMap) -> ServerFacts {
        let read = |name: &str| header_text(headers, name);
        let mut client_hints: Vec<(String, String)> = headers
            .iter()
            .filter_map(|(name, value)| {
                let hint = name.as_str().strip_prefix("sec-ch-ua")?;
                let hint = hint.strip_prefix('-').unwrap_or(hint);
                let value = clip(value.to_str().ok()?);
                Some((
                    if hint.is_empty() { "brands" } else { hint }.to_string(),
                    value,
                ))
            })
            .collect();
        client_hints.sort();
        ServerFacts {
            real_ip: read("x-real-ip"),
            forwarded_for: read("x-forwarded-for"),
            forwarded_host: read("x-forwarded-host"),
            forwarded_proto: read("x-forwarded-proto"),
            user_agent: read("user-agent"),
            accept_language: read("accept-language"),
            client_hints,
        }
    }
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(clip)
        .filter(|v| !v.is_empty())
}

/// `text` trimmed and cut to the longest value a line keeps.
pub(crate) fn clip(text: &str) -> String {
    text.trim().chars().take(MAX_HEADER_CHARS).collect()
}

/// One line of the audit log. A line never holds a request body, a query
/// string, a cookie or an `Authorization` value (D4).
#[derive(Clone, Debug, Serialize)]
pub struct Event {
    /// RFC 3339, UTC.
    pub at: String,
    pub event: EventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub actor: Actor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<LoginFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// The request path, never its query string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// On `command`: the verb, never its arguments (D12).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    /// On `command`: the project the verb names, as the caller sent it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The public address alone, on lines that do not carry every server fact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerFacts>,
    /// The tab that reported the device facts (D11). Source `client`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holder: Option<String>,
    /// On `device_profile_changed`: the normalized fields that changed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<&'static str>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub normalized: Option<normalize::Normalized>,
    /// The facts as the page reported them, so a line can be normalized
    /// again (D6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<facts::ClientFacts>,
}

impl Event {
    pub fn new(event: EventKind, device: Option<DeviceId>, actor: Actor) -> Event {
        Event {
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            event,
            device: device.map(|d| d.to_string()),
            actor,
            reason: None,
            method: None,
            path: None,
            status: None,
            verb: None,
            repo: None,
            ip: None,
            server: None,
            holder: None,
            changed: Vec::new(),
            normalized: None,
            client: None,
        }
    }
}

/// The audit log of one daemon store, and the key of its device cookies.
pub struct Audit {
    dir: Option<PathBuf>,
    key: OnceLock<Option<DeviceKey>>,
    /// Serializes appends and prunes; holds the day of the last prune.
    writer: Mutex<Option<i64>>,
    /// The last normalized facts of each device since the daemon started.
    last_facts: Mutex<HashMap<DeviceId, normalize::Normalized>>,
}

impl Audit {
    /// The audit log in `dir`, or a log that records nothing when `dir` is
    /// `None`.
    pub fn in_dir(dir: Option<PathBuf>) -> Audit {
        Audit {
            dir,
            key: OnceLock::new(),
            writer: Mutex::new(None),
            last_facts: Mutex::new(HashMap::new()),
        }
    }

    /// The log file, or `None` when the log is off.
    pub fn log_path(&self) -> Option<PathBuf> {
        self.dir.as_deref().map(log_path_in)
    }

    /// The device-cookie key. With `create`, a missing key is made, so a
    /// store gets the file only when a browser first needs a cookie. `None`
    /// when the log is off, or the key cannot be read or created (logged
    /// once).
    pub fn key(&self, create: bool) -> Option<&DeviceKey> {
        if let Some(key) = self.key.get() {
            return key.as_ref();
        }
        let path = device::key_path_in(self.dir.as_ref()?);
        if !create && !path.exists() {
            return None;
        }
        self.key
            .get_or_init(|| {
                DeviceKey::load_or_create(&path)
                    .inspect_err(|e| {
                        tracing::warn!(error = format!("{e:#}"), "device cookies are off")
                    })
                    .ok()
            })
            .as_ref()
    }

    /// Record a page's report of its device facts: `device_facts` the first
    /// time this daemon sees the device, `device_profile_changed` when the
    /// facts that do not change for one device did change, and nothing when
    /// the report says what the last one said.
    pub fn record_facts(
        &self,
        device: Option<DeviceId>,
        client: facts::ClientFacts,
        server: ServerFacts,
    ) {
        let normalized = normalize::normalize(&client, &server);
        let kind = match device {
            Some(id) => {
                let mut last = self.last_facts.lock().expect("audit facts lock poisoned");
                let kind = match last.get(&id) {
                    None => Some((EventKind::DeviceFacts, Vec::new())),
                    Some(prev) if prev.profile != normalized.profile => Some((
                        EventKind::DeviceProfileChanged,
                        normalize::changed_fields(prev, &normalized),
                    )),
                    Some(_) => None,
                };
                last.insert(id, normalized.clone());
                kind
            }
            None => Some((EventKind::DeviceFacts, Vec::new())),
        };
        let Some((kind, changed)) = kind else { return };
        let actor = if device.is_some() {
            Actor::Device
        } else {
            Actor::Unknown
        };
        let mut event = Event::new(kind, device, actor);
        event.holder = client.holder.as_ref().map(|h| h.as_str().to_string());
        event.changed = changed;
        event.normalized = Some(normalized);
        event.server = Some(server);
        event.client = Some(client);
        self.record(&event);
    }

    /// Append `event`. A failed write is logged and never fails the request.
    pub fn record(&self, event: &Event) {
        let Some(dir) = &self.dir else { return };
        if let Err(e) = self.append(dir, event) {
            tracing::warn!(error = format!("{e:#}"), "audit log write failed");
        }
    }

    fn append(&self, dir: &Path, event: &Event) -> Result<()> {
        let mut line = serde_json::to_vec(event).context("encoding an audit line")?;
        line.push(b'\n');
        let path = log_path_in(dir);
        let now = chrono::Utc::now().timestamp();
        let mut last_prune_day = self.writer.lock().expect("audit writer lock poisoned");
        if *last_prune_day != Some(now / 86_400) {
            prune(&path, now)?;
            *last_prune_day = Some(now / 86_400);
        }
        owner_only::create_owner_only_dir(dir)?;
        owner_only::append_owner_only(&path, &line)
    }
}

/// Remove lines older than [`RETENTION_SECS`] at `now`, then the oldest lines
/// while the file is over [`MAX_BYTES`]. A line whose time cannot be read is
/// kept. Rewrites the file only when a line goes.
pub fn prune(path: &Path, now: i64) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let cutoff = now - RETENTION_SECS;
    let mut kept: Vec<&str> = text
        .lines()
        .filter(|line| line_time(line).is_none_or(|at| at >= cutoff))
        .collect();
    let size = |lines: &[&str]| lines.iter().map(|l| l.len() as u64 + 1).sum::<u64>();
    if size(&kept) > MAX_BYTES {
        let mut total = size(&kept);
        let mut drop = 0;
        while total > PRUNE_TO_BYTES && drop < kept.len() {
            total -= kept[drop].len() as u64 + 1;
            drop += 1;
        }
        kept.drain(..drop);
    }
    if kept.len() == text.lines().count() {
        return Ok(());
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    owner_only::write_owner_only(path, out.as_bytes())
}

fn line_time(line: &str) -> Option<i64> {
    #[derive(serde::Deserialize)]
    struct At {
        at: String,
    }
    let at: At = serde_json::from_str(line).ok()?;
    chrono::DateTime::parse_from_rfc3339(&at.at)
        .ok()
        .map(|t| t.timestamp())
}

pub mod facts;
pub mod normalize;
pub mod read;

#[cfg(test)]
mod tests;
