//! The registry family (ADR-0036 amendment "the registry verbs", 2026-09-30):
//! `dir.list` and `project.add`. They name a daemon, not a repo, and a peer
//! serves them against its own disk and registry.

use std::path::Path;

use super::{ArgvError, Verb};

/// The longest `path` accepted, in bytes.
const MAX_PATH_BYTES: usize = crate::dir_list::MAX_PATH_BYTES;

/// The longest git name or email accepted, in bytes.
const MAX_GIT_IDENTITY_BYTES: usize = 256;

/// The error `daemon add` prints when git has no name and email for the
/// first commit. The daemon matches it to ask the browser for the two values.
pub const GIT_IDENTITY_MISSING: &str = "git has no name and email for commits on this \
     computer: run `git config --global user.name \"Your Name\"` and \
     `git config --global user.email you@example.com`, or pass --git-name and --git-email";

/// Whether `value` may be saved as a git `user.name`: not empty, not too long,
/// no control character, no space at either end, and no leading `-`.
pub fn valid_git_name(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= MAX_GIT_IDENTITY_BYTES
        && !value.starts_with('-')
        && !value.chars().any(char::is_control)
}

/// Whether `value` may be saved as a git `user.email`: a valid name with an `@`
/// that has text on both sides, and no space or angle bracket.
pub fn valid_git_email(value: &str) -> bool {
    valid_git_name(value)
        && !value.contains(|c: char| c.is_whitespace() || c == '<' || c == '>')
        && value
            .split_once('@')
            .is_some_and(|(user, host)| !user.is_empty() && !host.is_empty())
}

impl Verb {
    /// The registry family: no repo, routed by an optional `daemon` id, and
    /// relayed to a peer (unlike the host family).
    pub fn is_registry(self) -> bool {
        matches!(self, Verb::DirList | Verb::ProjectAdd)
    }
}

/// Compose `daemon add [--init [--create] [--git-name N --git-email E]] -- <path>`
/// for [`Verb::ProjectAdd`].
///
/// `path` is free text from the browser. It must be non-empty, absolute, at
/// most 4096 bytes, and hold no control character; the `--` keeps a path that
/// starts with `-` from being read as an option. `--init` is added only when
/// `init` is `true`, and `--create` only when `create` is `true` too.
/// `gitName` and `gitEmail` come together, with `init`, or not at all.
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
    let init = flag_param(payload, "init")?;
    let create = flag_param(payload, "create")?;
    if create && !init {
        return Err(ArgvError::BadParam("create"));
    }
    let mut argv = vec!["daemon".to_owned(), "add".to_owned()];
    if init {
        argv.push("--init".to_owned());
    }
    if create {
        argv.push("--create".to_owned());
    }
    if let Some((name, email)) = git_identity_params(payload)? {
        if !init {
            return Err(ArgvError::BadParam("gitName"));
        }
        argv.extend([
            "--git-name".to_owned(),
            name,
            "--git-email".to_owned(),
            email,
        ]);
    }
    argv.push("--".to_owned());
    argv.push(path.to_owned());
    Ok(argv)
}

/// An optional boolean param; absent or `null` is `false`.
fn flag_param(payload: &serde_json::Value, key: &'static str) -> Result<bool, ArgvError> {
    match payload.get(key) {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(v) => v.as_bool().ok_or(ArgvError::BadParam(key)),
    }
}

/// The optional `gitName` and `gitEmail` pair, each checked by its rule.
fn git_identity_params(payload: &serde_json::Value) -> Result<Option<(String, String)>, ArgvError> {
    let field = |key: &'static str, valid: fn(&str) -> bool| match payload.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => match v.as_str() {
            Some(s) if valid(s) => Ok(Some(s.to_owned())),
            _ => Err(ArgvError::BadParam(key)),
        },
    };
    match (
        field("gitName", valid_git_name)?,
        field("gitEmail", valid_git_email)?,
    ) {
        (None, None) => Ok(None),
        (Some(name), Some(email)) => Ok(Some((name, email))),
        (None, Some(_)) => Err(ArgvError::BadParam("gitName")),
        (Some(_), None) => Err(ArgvError::BadParam("gitEmail")),
    }
}

#[cfg(test)]
mod tests;
