//! The model list of `agy models`: one `id<TAB>label` line per model
//! (ADR-0086 D4). The list is plan-scoped, so Ralphy reads it and pins none.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// How long `agy models` may run.
const LIST_LIMIT: Duration = Duration::from_secs(30);

/// One model `agy` offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// The id `--model` takes, such as `gemini-3.8-flash-low`.
    pub id: String,
    /// The label `agy` shows, such as `Gemini 3.8 Flash (Low)`.
    pub label: String,
}

/// The models the signed-in plan offers, from `agy models` in the owned root.
/// An error when `agy` is not installed, is signed out, or fails.
pub fn fetch_catalog() -> Result<Vec<ModelEntry>> {
    let program =
        ralphy_proc_util::antigravity::locate_agy().context("the `agy` CLI is not installed")?;
    let root = ralphy_proc_util::antigravity::owned_root()?;
    list(&program, &root)
}

/// `agy models` of `program` in the owned `root`.
pub(crate) fn list(program: &Path, root: &Path) -> Result<Vec<ModelEntry>> {
    let mut cmd = crate::command::agy_command(program);
    cmd.arg(crate::command::root_flag(root))
        .arg("models")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = ralphy_adapter_support::run_headless(cmd, "", LIST_LIMIT)
        .context("failed to start `agy models`")?;
    let code = out.exit.and_then(|status| status.code());
    if crate::auth::models_reads_logged_out(code, &out.stderr) {
        bail!("{}", crate::auth::auth_message(root));
    }
    if out.timed_out || code != Some(0) {
        bail!("`agy models` failed: {}", out.stderr.trim());
    }
    Ok(parse(&out.stdout))
}

/// The models in `stdout`, in order. A line with no tab, or with an empty id
/// or label, is skipped.
pub(crate) fn parse(stdout: &str) -> Vec<ModelEntry> {
    stdout
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(id, label)| (id.trim(), label.trim()))
        .filter(|(id, label)| !id.is_empty() && !label.is_empty())
        .map(|(id, label)| ModelEntry {
            id: id.to_owned(),
            label: label.to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
