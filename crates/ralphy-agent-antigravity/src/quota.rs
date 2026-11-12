//! The quota read-out of `agy` and the reset time of a limit (ADR-0086 D6).
//! The read-out is a slash command answered without a model call.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "used once the Agent impl lands")
)]

use std::ffi::OsString;
use std::path::Path;

use serde::Deserialize;

/// The quota group of the Gemini models.
const GEMINI_GROUP: &str = "Gemini Models";

/// The quota group of the Claude and GPT models.
const THIRD_PARTY_GROUP: &str = "Claude and GPT models";

/// The quota group of the Argon models, whose quota is separate.
const ARGON_GROUP: &str = "Gemini 4 Argon";

/// The argv of the quota read-out in the owned `root`. No `--model`: the
/// command is answered before any model call.
pub(crate) fn quota_args(root: &Path) -> Vec<OsString> {
    let mut root_flag = OsString::from("--gemini_dir=");
    root_flag.push(root);
    vec![
        root_flag,
        "-p".into(),
        "/quota".into(),
        "--output-format".into(),
        "json".into(),
    ]
}

/// The reset time (RFC 3339, UTC) of a limit hit by `model`: the `reset_time`
/// of the buckets of its group that are spent (`remaining_fraction` 0), the
/// latest when several are. `None` when `model` is unset or unknown, when no
/// bucket is spent, or when `stdout` is not the read-out.
///
/// The reset time of a bucket that is not spent means nothing: it moves
/// between reads (spike section 7). The times are UTC in one format, so the
/// later of two is the greater string.
pub(crate) fn reset_time(stdout: &str, model: Option<&str>) -> Option<String> {
    let wanted = group_of(model?)?;
    let report: Report = serde_json::from_str(stdout.trim()).ok()?;
    report
        .command
        .data
        .groups
        .into_iter()
        .filter(|group| group.name == wanted)
        .flat_map(|group| group.buckets)
        .filter(|bucket| bucket.remaining_fraction <= 0.0)
        .filter_map(|bucket| bucket.reset_time)
        .max()
}

/// The quota group a model id bills against.
fn group_of(id: &str) -> Option<&'static str> {
    if id.starts_with("gemini-4-argon") {
        Some(ARGON_GROUP)
    } else if id.starts_with("gemini-") {
        Some(GEMINI_GROUP)
    } else if id.starts_with("claude-") || id.starts_with("gpt-oss-") {
        Some(THIRD_PARTY_GROUP)
    } else {
        None
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Report {
    command: Command,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Command {
    data: Data,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Data {
    groups: Vec<Group>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Group {
    name: String,
    buckets: Vec<Bucket>,
}

#[derive(Debug, Deserialize)]
struct Bucket {
    remaining_fraction: f64,
    reset_time: Option<String>,
}

#[cfg(test)]
mod tests;
