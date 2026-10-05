//! Reading the audit log back for the workbench (ADR-0074 D9): the devices it
//! names, and one device's lines. The file is at most 20 MiB, so a read goes
//! through all of it each time the Devices section opens.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::device::DeviceId;
use crate::registry::{self, RegistryStore};

/// The most lines one read returns.
pub const MAX_EVENTS: usize = 200;

/// One device as the log knows it. The description fields come from its
/// latest `device_facts` or `device_profile_changed` line.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DeviceSummary {
    pub device: String,
    /// The device that asked.
    pub this: bool,
    pub first_seen: String,
    pub last_seen: String,
    /// How many lines name the device.
    pub events: u32,
    /// The public address of its latest line that has one.
    pub ip: Option<String>,
    pub os: Option<String>,
    pub os_version: Option<String>,
    pub browser: Option<String>,
    pub browser_version: Option<String>,
    pub form: Option<String>,
    pub model: Option<String>,
    pub gpu: Option<String>,
}

fn lines(path: &Path) -> Result<Vec<Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

fn text(v: &Value, pointer: &str) -> Option<String> {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Every device the log names, the latest seen first.
pub fn devices(path: &Path, this: Option<DeviceId>) -> Result<Vec<DeviceSummary>> {
    let this = this.map(|d| d.to_string());
    let mut by_id: HashMap<String, DeviceSummary> = HashMap::new();
    for line in lines(path)? {
        let (Some(id), Some(at)) = (text(&line, "/device"), text(&line, "/at")) else {
            continue;
        };
        let d = by_id.entry(id.clone()).or_insert_with(|| DeviceSummary {
            this: this.as_deref() == Some(id.as_str()),
            device: id,
            first_seen: at.clone(),
            ..DeviceSummary::default()
        });
        d.last_seen = at;
        d.events += 1;
        if let Some(ip) = text(&line, "/server/real_ip").or_else(|| text(&line, "/ip")) {
            d.ip = Some(ip);
        }
        if line.get("normalizer").is_some() {
            d.os = text(&line, "/os/value");
            d.os_version = text(&line, "/os_version/value");
            d.browser = text(&line, "/browser/value");
            d.browser_version = text(&line, "/browser_version");
            d.form = text(&line, "/form/value");
            d.model = text(&line, "/model_hint/value");
            d.gpu = text(&line, "/gpu/model");
        }
    }
    let mut out: Vec<DeviceSummary> = by_id.into_values().collect();
    out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(a.device.cmp(&b.device)));
    Ok(out)
}

/// The latest `limit` lines that name `device`, newest first.
pub fn events(path: &Path, device: &str, limit: usize) -> Result<Vec<Value>> {
    let mut out: Vec<Value> = lines(path)?
        .into_iter()
        .filter(|l| l.get("device").and_then(Value::as_str) == Some(device))
        .collect();
    out.reverse();
    out.truncate(limit.min(MAX_EVENTS));
    Ok(out)
}

/// Add `repo_name`, the project name the registry gives (`registry::project_name`),
/// to each line whose `repo` is a registered project. A line keeps `repo` as it
/// was recorded; a project this daemon does not register (a peer's, or one
/// removed since) gets no name.
pub fn name_repos(events: &mut [Value], store: &RegistryStore) {
    for event in events {
        let Some(slug) = event.get("repo").and_then(Value::as_str) else {
            continue;
        };
        let Some(entry) = store.entry(slug) else {
            continue;
        };
        let name = registry::project_name(slug, &entry.path);
        if let Some(line) = event.as_object_mut() {
            line.insert("repo_name".to_string(), Value::String(name));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registered_repo_gets_its_project_name_and_another_keeps_none() {
        let mut store = RegistryStore::default();
        store.upsert("path-0123456789abcdef", "C:\\src\\widget");
        let mut events = vec![
            serde_json::json!({"event": "command", "repo": "path-0123456789abcdef"}),
            serde_json::json!({"event": "command", "repo": "nope"}),
            serde_json::json!({"event": "login_ok"}),
        ];
        name_repos(&mut events, &store);
        assert_eq!(events[0]["repo_name"], "widget");
        assert_eq!(
            events[0]["repo"], "path-0123456789abcdef",
            "the recorded value stays"
        );
        assert!(events[1].get("repo_name").is_none());
        assert!(events[2].get("repo_name").is_none());
    }

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn devices_are_listed_latest_first_with_their_latest_facts_and_address() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon-audit.jsonl");
        let lines = [
            format!(
                r#"{{"at":"2026-10-01T10:00:00Z","event":"device_facts","device":"{A}","actor":"device","normalizer":1,"os":{{"value":"ios","source":"ua"}},"server":{{"real_ip":"203.0.113.1"}}}}"#
            ),
            format!(
                r#"{{"at":"2026-10-02T10:00:00Z","event":"action","device":"{B}","actor":"device","ip":"203.0.113.2"}}"#
            ),
            format!(
                r#"{{"at":"2026-10-03T10:00:00Z","event":"device_profile_changed","device":"{A}","actor":"device","normalizer":1,"os":{{"value":"ipados","source":"inferred"}}}}"#
            ),
            format!(
                r#"{{"at":"2026-10-04T10:00:00Z","event":"action","device":"{A}","actor":"device","ip":"203.0.113.3"}}"#
            ),
            r#"{"at":"2026-10-04T11:00:00Z","event":"login_failed","actor":"unknown"}"#.to_string(),
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let list = devices(&path, None).unwrap();
        let ids: Vec<&str> = list.iter().map(|d| d.device.as_str()).collect();
        assert_eq!(ids, [A, B], "a line with no device names no device");
        let a = &list[0];
        assert_eq!(a.os.as_deref(), Some("ipados"), "the latest facts win");
        assert_eq!(
            a.ip.as_deref(),
            Some("203.0.113.3"),
            "the latest address wins"
        );
        assert_eq!(a.events, 3);
        assert_eq!(a.first_seen, "2026-10-01T10:00:00Z");
    }
}
