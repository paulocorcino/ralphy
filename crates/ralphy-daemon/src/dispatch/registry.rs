//! The registry family (ADR-0036 amendment "the registry verbs", 2026-09-30):
//! `dir.list` and `project.add`. They name a daemon, not a repo, and a peer
//! serves them against its own disk and registry.

use std::path::Path;

use super::{ArgvError, Verb};

/// The longest `path` accepted, in bytes.
const MAX_PATH_BYTES: usize = crate::dir_list::MAX_PATH_BYTES;

impl Verb {
    /// The registry family: no repo, routed by an optional `daemon` id, and
    /// relayed to a peer (unlike the host family).
    pub fn is_registry(self) -> bool {
        matches!(self, Verb::DirList | Verb::ProjectAdd)
    }
}

/// Compose `daemon add [--init [--create]] -- <path>` for [`Verb::ProjectAdd`].
///
/// `path` is free text from the browser. It must be non-empty, absolute, at
/// most 4096 bytes, and hold no control character; the `--` keeps a path that
/// starts with `-` from being read as an option. `--init` is added only when
/// `init` is `true`, and `--create` only when `create` is `true` too.
pub fn project_add_argv(payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let path = payload
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or(ArgvError::BadParam("path"))?;
    let ok = !path.is_empty()
        && path.len() <= MAX_PATH_BYTES
        && !path.chars().any(char::is_control)
        && Path::new(path).is_absolute();
    if !ok {
        return Err(ArgvError::BadParam("path"));
    }
    let init = match payload.get("init") {
        None | Some(serde_json::Value::Null) => false,
        Some(v) => v.as_bool().ok_or(ArgvError::BadParam("init"))?,
    };
    let create = match payload.get("create") {
        None | Some(serde_json::Value::Null) => false,
        Some(v) => v.as_bool().ok_or(ArgvError::BadParam("create"))?,
    };
    if create && !init {
        return Err(ArgvError::BadParam("create"));
    }
    let mut argv = vec!["daemon".to_string(), "add".to_string()];
    if init {
        argv.push("--init".to_string());
    }
    if create {
        argv.push("--create".to_string());
    }
    argv.push("--".to_string());
    argv.push(path.to_string());
    Ok(argv)
}

#[cfg(test)]
mod tests;
