//! The owned root's config files (ADR-0086 D8, D9, D13), and the repository
//! hooks file a run warns about (D10).
//!
//! `agy` reads `<root>/config/hooks.json` as its global hook layer: a top-level
//! key per named hook, a tool event wrapping its handlers in a `matcher`, the
//! other events as a flat list. A global `deny` wins over a repository `allow`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

/// The name of Ralphy's hook in the hooks file.
const HOOK_NAME: &str = "ralphy";

/// The events the status hook runs on, in the order `agy` fires them.
const STATUS_EVENTS: [&str; 6] = [
    "SessionStart",
    "PreInvocation",
    "PreToolUse",
    "PostToolUse",
    "PostInvocation",
    "Stop",
];

/// The events whose handlers sit under a tool `matcher`.
const TOOL_EVENTS: [&str; 2] = ["PreToolUse", "PostToolUse"];

/// The folders a repository's own `hooks.json` is read from.
const WORKSPACE_HOOK_DIRS: [&str; 3] = [".agents", ".agent", "_agents"];

/// Write the hooks file and turn `swarm` off in the owned `root`, with the
/// running `ralphy` as the hook program.
pub(crate) fn ensure(root: &Path) -> Result<()> {
    let ralphy = ralphy_proc_util::exe::running_exe()?;
    let config = root.join("config");
    fs::create_dir_all(&config).with_context(|| {
        format!(
            "creating the Antigravity config folder {}",
            config.display()
        )
    })?;
    let hooks = serde_json::to_string_pretty(&hooks_document(&ralphy)?)?;
    write_if_changed(&config.join("hooks.json"), &format!("{hooks}\n"))?;
    turn_swarm_off(&config.join("config.json"))
}

/// The hooks file: the guard first on every tool, then the status hook on the
/// six events. A `PreToolUse` handler that fails denies the tool (D9).
fn hooks_document(ralphy: &Path) -> Result<Value> {
    let handler = |args: &[&str]| -> Result<Value> {
        let command = ralphy_proc_util::antigravity::hook_command(ralphy, args)?;
        Ok(json!({ "type": "command", "command": command }))
    };
    let guard = handler(&["hook", "guard", "--vendor", "antigravity"])?;
    let status = handler(&["hook", "status", "antigravity"])?;
    let mut events = Map::new();
    for event in STATUS_EVENTS {
        let mut handlers = Vec::new();
        if event == "PreToolUse" {
            handlers.push(guard.clone());
        }
        handlers.push(status.clone());
        let entry = if TOOL_EVENTS.contains(&event) {
            json!([{ "matcher": "*", "hooks": handlers }])
        } else {
            Value::Array(handlers)
        };
        events.insert(event.to_owned(), entry);
    }
    Ok(json!({ HOOK_NAME: events }))
}

/// Set `plugins.swarm.enabled` to `false` in `config.json`, keeping every other
/// key: `agy` writes the file itself, with `swarm` on, at its first run (D13).
fn turn_swarm_off(path: &Path) -> Result<()> {
    let mut config = match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("reading the Antigravity config {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => {
            return Err(e).with_context(|| format!("reading {}", path.display()));
        }
    };
    let swarm = object_at(&mut config, &["plugins", "swarm"])
        .with_context(|| format!("the Antigravity config {} is not an object", path.display()))?;
    swarm.insert("enabled".to_owned(), Value::Bool(false));
    let text = serde_json::to_string_pretty(&config)?;
    write_if_changed(path, &format!("{text}\n"))
}

/// The object at `keys` inside `value`, made where it is missing. `None` when
/// `value` or a key on the way holds something other than an object.
fn object_at<'a>(value: &'a mut Value, keys: &[&str]) -> Option<&'a mut Map<String, Value>> {
    let mut object = value.as_object_mut()?;
    for key in keys {
        object = object
            .entry((*key).to_owned())
            .or_insert_with(|| json!({}))
            .as_object_mut()?;
    }
    Some(object)
}

/// Write `text` to `path` only when the file differs, so a run does not touch
/// a root that is already right.
fn write_if_changed(path: &Path, text: &str) -> Result<()> {
    if fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// The nearest repository hooks file `agy` will also run, from `cwd` up to
/// `repo_root`: `hooks.json` in `.agents/`, `.agent/` or `_agents/` (D10).
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used once the Agent impl lands")
)]
pub(crate) fn workspace_hooks_file(cwd: &Path, repo_root: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .take_while(|dir| dir.starts_with(repo_root))
        .flat_map(|dir| WORKSPACE_HOOK_DIRS.map(|name| dir.join(name).join("hooks.json")))
        .find(|file| file.is_file())
}

#[cfg(test)]
mod tests;
