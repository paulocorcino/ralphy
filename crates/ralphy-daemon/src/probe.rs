//! The scroll probe (ADR-0084): the page reports one record per console
//! scroll as the verb `probe.scroll`, and the daemon appends each record as a
//! JSON line to `probe/scroll.jsonl` in its store. Built only with the Cargo
//! feature `scroll-probe` (D1); a daemon without it answers `unknown verb`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::device::DeviceId;
use crate::owner_only;

/// The verb name. It is not in `dispatch::Verb` (D2).
pub(crate) const VERB: &str = "probe.scroll";

/// The most records one command may carry (D4).
const MAX_RECORDS: usize = 20;

/// Past this size the file is renamed to `scroll.1.jsonl` (D5).
const MAX_BYTES: u64 = 5 * 1024 * 1024;

/// How the scroll was made.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Input {
    Touch,
    Glide,
    Wheel,
}

/// The renderer that drew the console.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Renderer {
    Webgl,
    Dom,
}

/// Where the console was on the page.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum View {
    Stage,
    Maximized,
    Column,
    Tab,
    Full,
}

/// One scroll, as the page measured it (D4). Only numbers and closed values:
/// no text of the console.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    input: Input,
    /// From the first move to the last, in ms.
    duration_ms: f64,
    frames: u32,
    /// Frames over 25 ms.
    slow_frames: u32,
    /// Slow frames in which the terminal also wrote output.
    slow_with_output: u32,
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    /// The time of the page's own scroll steps, summed.
    step_ms: f64,
    /// Bytes of output the terminal wrote during the scroll.
    output_bytes: u64,
    renderer: Renderer,
    rows: u32,
    cols: u32,
    history: u32,
    view: View,
    dpr: f64,
}

/// What the payload of `probe.scroll` holds.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    records: Vec<Record>,
}

/// One line of the file: the record, with what the daemon adds (D5).
#[derive(Serialize)]
struct Line<'a> {
    at: String,
    device: Option<String>,
    #[serde(flatten)]
    record: &'a Record,
}

/// The file the records go to, inside the daemon's store `dir`.
fn path_in(dir: &Path) -> PathBuf {
    dir.join("probe").join("scroll.jsonl")
}

/// The records of `payload`, or why they are refused.
fn records_of(payload: &serde_json::Value) -> Result<Vec<Record>, String> {
    let payload = Payload::deserialize(payload).map_err(|e| format!("not a scroll record: {e}"))?;
    if payload.records.len() > MAX_RECORDS {
        return Err(format!("more than {MAX_RECORDS} records"));
    }
    Ok(payload.records)
}

/// Append `records` to the file in `dir`, renaming a full file first.
fn append(dir: &Path, records: &[Record], device: Option<&DeviceId>) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let path = path_in(dir);
    let parent = path.parent().context("the probe file has no folder")?;
    owner_only::create_owner_only_dir(parent)?;
    rotate(&path)?;
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let device = device.map(ToString::to_string);
    let mut out = Vec::new();
    for record in records {
        let line = Line {
            at: at.clone(),
            device: device.clone(),
            record,
        };
        serde_json::to_writer(&mut out, &line).context("encoding a scroll record")?;
        out.push(b'\n');
    }
    owner_only::append_owner_only(&path, &out)
}

/// Rename `path` to `scroll.1.jsonl` when it is over [`MAX_BYTES`].
fn rotate(path: &Path) -> Result<()> {
    let size = match std::fs::metadata(path) {
        Ok(meta) => meta.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).context("reading the size of the probe file"),
    };
    if size <= MAX_BYTES {
        return Ok(());
    }
    std::fs::rename(path, path.with_file_name("scroll.1.jsonl"))
        .context("renaming the full probe file")
}

/// The one reply to `probe.scroll`. An empty list is the page asking whether
/// the probe exists (D3): it is answered `ok` and writes nothing.
pub(crate) async fn reply(
    payload: serde_json::Value,
    device: Option<DeviceId>,
    dir: PathBuf,
) -> serde_json::Value {
    let records = match records_of(&payload) {
        Ok(records) => records,
        Err(message) => return serde_json::json!({ "status": "error", "message": message }),
    };
    let written =
        tokio::task::spawn_blocking(move || append(&dir, &records, device.as_ref())).await;
    match written {
        Ok(Ok(())) => serde_json::json!({ "status": "ok" }),
        Ok(Err(e)) => {
            tracing::warn!(error = format!("{e:#}"), "scroll probe write failed");
            serde_json::json!({ "status": "error", "message": "the probe file could not be written" })
        }
        Err(e) => {
            tracing::warn!(error = %e, "scroll probe task failed");
            serde_json::json!({ "status": "error", "message": "the probe file could not be written" })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> serde_json::Value {
        serde_json::json!({
            "input": "touch", "duration_ms": 812.5, "frames": 49, "slow_frames": 3,
            "slow_with_output": 1, "p50_ms": 16.7, "p95_ms": 33.1, "max_ms": 61.0,
            "step_ms": 12.4, "output_bytes": 2048, "renderer": "dom", "rows": 40,
            "cols": 90, "history": 1200, "view": "maximized", "dpr": 3.0
        })
    }

    #[tokio::test]
    async fn a_record_is_one_line_with_the_time_and_the_device() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let device = DeviceId::mint();
        let payload = serde_json::json!({ "records": [record(), record()] });
        let reply = reply(payload, Some(device), dir.path().to_path_buf()).await;
        assert_eq!(reply["status"], "ok", "{reply}");
        let text = std::fs::read_to_string(path_in(dir.path())).expect("the probe file");
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).expect("a JSON line"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["device"], device.to_string());
        assert_eq!(lines[0]["slow_frames"], 3);
        assert!(
            lines[0]["at"].as_str().is_some_and(|at| at.ends_with('Z')),
            "{}",
            lines[0]
        );
    }

    #[tokio::test]
    async fn the_empty_ask_is_ok_and_writes_nothing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let reply = reply(
            serde_json::json!({ "records": [] }),
            None,
            dir.path().to_path_buf(),
        )
        .await;
        assert_eq!(reply["status"], "ok", "{reply}");
        assert!(!path_in(dir.path()).exists());
    }

    #[tokio::test]
    async fn a_record_out_of_shape_is_refused() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut text = record();
        text["line"] = serde_json::json!("PS C:\\> secret");
        let mut renderer = record();
        renderer["renderer"] = serde_json::json!("canvas");
        let too_many = serde_json::json!({ "records": vec![record(); MAX_RECORDS + 1] });
        for payload in [
            serde_json::json!({ "records": [text] }),
            serde_json::json!({ "records": [renderer] }),
            too_many,
        ] {
            let reply = reply(payload, None, dir.path().to_path_buf()).await;
            assert_eq!(reply["status"], "error", "{reply}");
        }
        assert!(!path_in(dir.path()).exists());
    }

    #[test]
    fn a_full_file_is_renamed_before_the_next_line() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = path_in(dir.path());
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("the probe folder");
        let file = std::fs::File::create(&path).expect("the probe file");
        file.set_len(MAX_BYTES + 1).expect("a full file");
        drop(file);
        let one: Record = serde_json::from_value(record()).expect("a record");
        append(dir.path(), &[one], None).expect("the append");
        assert!(path.with_file_name("scroll.1.jsonl").exists());
        assert_eq!(
            std::fs::read_to_string(&path)
                .expect("the new file")
                .lines()
                .count(),
            1
        );
    }
}
