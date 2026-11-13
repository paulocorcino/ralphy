//! The quota read-out of `agy` and the reset time of a limit (ADR-0086 D6).
//! The read-out is a slash command answered without a model call.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use ralphy_core::ResetAt;
use serde::Deserialize;

/// How long the quota read-out may run.
const READ_LIMIT: Duration = Duration::from_secs(30);

/// The quota group of the Gemini models.
const GEMINI_GROUP: &str = "Gemini Models";

/// The quota group of the Claude and GPT models.
const THIRD_PARTY_GROUP: &str = "Claude and GPT models";

/// The quota group of the Argon models, whose quota is separate.
const ARGON_GROUP: &str = "Gemini 4 Argon";

/// The argv of the quota read-out in the owned `root`. No `--model`: the
/// command is answered before any model call.
pub(crate) fn quota_args(root: &Path) -> Vec<OsString> {
    vec![
        crate::command::root_flag(root),
        "-p".into(),
        "/quota".into(),
        "--output-format".into(),
        "json".into(),
    ]
}

/// The reset of a limit `model` hit, from the quota read-out of `program` in
/// the owned `root`. `None` when the read-out fails or names no reset: the
/// run clock then waits its own window.
pub(crate) fn read_reset(program: &Path, root: &Path, model: Option<&str>) -> Option<ResetAt> {
    let mut cmd = crate::command::agy_command(program);
    cmd.args(quota_args(root))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = match ralphy_adapter_support::run_headless(cmd, "", READ_LIMIT) {
        Ok(out) => out,
        Err(e) => {
            tracing::warn!("antigravity: the quota read-out did not run: {e:#}");
            return None;
        }
    };
    let text = reset_time(&out.stdout, model)?;
    let at = chrono::DateTime::parse_from_rfc3339(&text).ok();
    Some(ResetAt { text, at })
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
