//! The argv of the host verbs (ADR-0036 amendment 2026-09-29). Unlike every
//! other verb they carry free text (a destination, a key path, a name), so
//! each value is checked here before `ssh` can read it as an option.

use std::path::Path;

use zeroize::Zeroizing;

use super::{ArgvError, Verb};

/// A destination or host name: an SSH config alias, `user@host` or
/// `ssh://user@host:port`, or a daemon id.
fn destination<'a>(
    payload: &'a serde_json::Value,
    key: &'static str,
) -> Result<&'a str, ArgvError> {
    let value = payload
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or(ArgvError::BadParam(key))?;
    let ok = (1..=255).contains(&value.chars().count())
        && !value.starts_with('-')
        && !value.chars().any(|c| c.is_whitespace() || c.is_control());
    ok.then_some(value).ok_or(ArgvError::BadParam(key))
}

/// An optional string field: absent, `null` and `""` all mean "not given".
fn optional<'a>(
    payload: &'a serde_json::Value,
    key: &'static str,
) -> Result<Option<&'a str>, ArgvError> {
    match payload.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => match v.as_str() {
            Some("") => Ok(None),
            Some(s) => Ok(Some(s)),
            None => Err(ArgvError::BadParam(key)),
        },
    }
}

fn identity(payload: &serde_json::Value) -> Result<Option<&str>, ArgvError> {
    let Some(path) = optional(payload, "identity")? else {
        return Ok(None);
    };
    let ok =
        path.len() <= 4096 && !path.chars().any(char::is_control) && Path::new(path).is_absolute();
    ok.then_some(Some(path))
        .ok_or(ArgvError::BadParam("identity"))
}

/// The CLI's `validate_name` stays the authority; this only keeps the value a
/// single, non-option argument.
fn name(payload: &serde_json::Value) -> Result<Option<&str>, ArgvError> {
    let Some(name) = optional(payload, "name")? else {
        return Ok(None);
    };
    let ok = name.chars().count() <= 64
        && !name.starts_with('-')
        && !name.chars().any(|c| c.is_whitespace() || c.is_control());
    ok.then_some(Some(name)).ok_or(ArgvError::BadParam("name"))
}

/// `SHA256:` and the 43 characters of an unpadded base64 SHA-256.
fn fingerprint(payload: &serde_json::Value) -> Result<&str, ArgvError> {
    let fp = payload
        .get("fingerprint")
        .and_then(|v| v.as_str())
        .ok_or(ArgvError::BadParam("fingerprint"))?;
    let ok = fp.strip_prefix("SHA256:").is_some_and(|b64| {
        b64.len() == 43
            && b64
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
    });
    ok.then_some(fp).ok_or(ArgvError::BadParam("fingerprint"))
}

fn rotate_token(payload: &serde_json::Value) -> Result<bool, ArgvError> {
    match payload.get("rotate_token") {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(v) => v.as_bool().ok_or(ArgvError::BadParam("rotate_token")),
    }
}

/// The longest password a host verb accepts.
const MAX_PASSWORD: usize = 1024;

/// The operator's password for a host verb that signs in, when the payload has
/// a non-empty one. It never goes into the argv: [`host_argv`] only adds
/// `--password-stdin`, and the caller writes it to the child's standard input.
pub fn host_password(
    verb: Verb,
    payload: &serde_json::Value,
) -> Result<Option<Zeroizing<String>>, ArgvError> {
    let Some(value) = payload.get("password") else {
        return Ok(None);
    };
    if !matches!(verb, Verb::HostCheck | Verb::HostAdd | Verb::HostInstall) {
        return Err(ArgvError::BadParam("password"));
    }
    let text = value.as_str().ok_or(ArgvError::BadParam("password"))?;
    if text.is_empty() {
        return Ok(None);
    }
    if text.len() > MAX_PASSWORD || text.contains('\0') {
        return Err(ArgvError::BadParam("password"));
    }
    Ok(Some(Zeroizing::new(text.to_string())))
}

/// Compose `ralphy host …` for a host verb. Any other verb, or any value
/// that fails its check, yields [`ArgvError`] and no argv.
pub fn host_argv(verb: Verb, payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let mut argv: Vec<String> = vec!["host".to_string()];
    let mut push = |parts: &[&str]| argv.extend(parts.iter().map(|s| s.to_string()));
    match verb {
        Verb::HostAliases => push(&["aliases"]),
        Verb::HostKey => push(&["key", destination(payload, "destination")?]),
        Verb::HostTrust => push(&[
            "trust",
            destination(payload, "destination")?,
            "--fingerprint",
            fingerprint(payload)?,
        ]),
        Verb::HostCheck | Verb::HostAdd => {
            let sub = if verb == Verb::HostCheck {
                "check"
            } else {
                "add"
            };
            push(&[sub, destination(payload, "destination")?, "--json"]);
            if let Some(path) = identity(payload)? {
                push(&["--identity", path]);
            }
            if let Some(name) = name(payload)? {
                push(&["--name", name]);
            }
            if host_password(verb, payload)?.is_some() {
                push(&["--password-stdin"]);
            }
        }
        // The dialog sends its whole payload; a name belongs to `add` only.
        Verb::HostInstall => {
            push(&["install", destination(payload, "destination")?, "--json"]);
            if let Some(path) = identity(payload)? {
                push(&["--identity", path]);
            }
            if host_password(verb, payload)?.is_some() {
                push(&["--password-stdin"]);
            }
        }
        Verb::HostRemove => {
            push(&["remove", destination(payload, "host")?, "--json"]);
            if rotate_token(payload)? {
                push(&["--rotate-token"]);
            }
        }
        _ => return Err(ArgvError::BadParam("verb")),
    }
    Ok(argv)
}

#[cfg(test)]
mod tests;
