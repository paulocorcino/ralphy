//! Run-time resolution of the run knobs (ADR-0010): a per-run flag, then the
//! persisted `settings.json` value, then a default.

use anyhow::{bail, Result};
use ralphy_core::{BranchMode, Effort};

/// Resolve a vendor-neutral optional model knob (ADR-0010). Precedence: `flag`,
/// then the persisted `settings.json` value, then `None` (the adapter resolves
/// its own default). Empty strings on either source are treated as unset.
pub fn resolve_optional_model(flag: Option<String>, persisted: Option<String>) -> Option<String> {
    flag.filter(|s| !s.is_empty())
        .or_else(|| persisted.filter(|s| !s.is_empty()))
}

/// Resolve the OpenCode execution model from the per-run flag and the
/// persisted setting (ADR-0010). Precedence: `exec_model` flag > persisted
/// `opencode.model` > `None` (OpenCode resolves its own default). Empty
/// strings are treated as unset.
pub fn resolve_opencode_model(
    exec_model: Option<String>,
    persisted: Option<String>,
) -> Option<String> {
    resolve_optional_model(exec_model, persisted)
}

/// Resolve a string-valued run knob (ADR-0010). Precedence: per-run `flag` >
/// persisted `settings.json` value > hardcoded `default`. Empty strings on
/// either source are treated as unset so they fall through to the next slot.
pub fn resolve_str(flag: Option<String>, persisted: Option<String>, default: &str) -> String {
    flag.filter(|s| !s.is_empty())
        .or_else(|| persisted.filter(|s| !s.is_empty()))
        .unwrap_or_else(|| default.to_owned())
}

/// Resolve one phase's neutral effort. Persisted strings are parsed even when
/// supplied outside `config set`, so hand-edited settings fail before adapter construction.
pub fn resolve_effort(
    flag: Option<Effort>,
    persisted: Option<String>,
    default: Option<Effort>,
) -> Result<Option<Effort>> {
    if let Some(flag) = flag {
        return Ok(Some(flag));
    }
    match persisted.filter(|value| !value.is_empty()) {
        Some(value) => value
            .parse::<Effort>()
            .map(Some)
            .map_err(anyhow::Error::msg),
        None => Ok(default),
    }
}

/// Resolve the effective queue assignee filter. Precedence:
/// `--assignee X` (non-empty) > `--no-assignee` (forces `None`) >
/// persisted `queue.assignee` (non-empty) > `None` (no filter). Empty strings on
/// either source are treated as unset, matching [`resolve_str`].
pub fn resolve_assignee(
    flag: Option<&str>,
    no_assignee: bool,
    persisted: Option<&str>,
) -> Option<String> {
    if let Some(a) = flag.filter(|s| !s.is_empty()) {
        return Some(a.to_owned());
    }
    if no_assignee {
        return None;
    }
    persisted.filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Resolve a `u64`-valued run knob (ADR-0010). Precedence: per-run `flag` >
/// persisted `settings.json` value > hardcoded `default`.
pub fn resolve_u64(flag: Option<u64>, persisted: Option<u64>, default: u64) -> u64 {
    flag.or(persisted).unwrap_or(default)
}

/// Resolve the effective Remote Control switch (#148). Precedence:
/// `--remote-control` > `--no-remote-control` > persisted `remote_control` >
/// `false` (OFF by default).
pub fn resolve_remote_control(
    remote_control: bool,
    no_remote_control: bool,
    persisted: Option<bool>,
) -> bool {
    if remote_control {
        true
    } else if no_remote_control {
        false
    } else {
        persisted.unwrap_or(false)
    }
}

/// Parse a persisted/`config set` `branch_mode` string into the core enum.
/// Accepts the lowercase canonical forms `"new"` / `"current"`; any other value
/// is a hard error so an invalid setting fails loud rather than silently
/// resolving to a default. The single validation path shared by `config set`
/// and run-time resolution keeps `ralphy-core`'s [`BranchMode`] serde-free.
pub fn parse_branch_mode(value: &str) -> Result<BranchMode> {
    match value {
        "new" => Ok(BranchMode::New),
        "current" => Ok(BranchMode::Current),
        other => bail!("branch_mode must be 'new' or 'current', got '{other}'"),
    }
}

#[cfg(test)]
mod tests;
