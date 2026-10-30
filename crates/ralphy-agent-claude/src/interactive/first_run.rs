//! The first-run gates of an interactive session: the workspace-trust dialog
//! and the onboarding wizard, pre-cleared in `~/.claude.json`.

use std::path::Path;

use crate::usage::dirs_home;

/// Pre-clear the first-run gates that block an *interactive* Claude session for
/// `repo_root`: the workspace-trust dialog AND the theme/onboarding wizard. The
/// headless `-p` planning path is exempt from both, but a live session stalls on
/// either forever waiting for a keypress — so an autonomous orchestrator must
/// grant up front what the operator would otherwise click. (Observed in the wild:
/// on a profile with `hasCompletedOnboarding=false`, every live exec hung at
/// "Choose the text style…" and silently burned the whole budget.) Best-effort:
/// reads `~/.claude.json`, sets the flags, and writes it back, preserving
/// everything else. A failure here just means the live session may stall
/// (surfaced as a Timeout), never a crash.
pub(super) fn ensure_interactive_session_ready(repo_root: &Path) {
    let Some(home) = dirs_home() else {
        return;
    };
    let cfg_path = home.join(".claude.json");
    let root = std::fs::read_to_string(&cfg_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    // Claude keys projects by the cwd it is launched with; we launch it at
    // `repo_root`, whose display form uses forward slashes on every platform.
    let key = repo_root.to_string_lossy().replace('\\', "/");
    let updated = with_onboarding_completed(with_workspace_trusted(root, &key));
    if let Ok(s) = serde_json::to_string_pretty(&updated) {
        let _ = std::fs::write(&cfg_path, s);
    }
}

/// Set `projects[key].hasTrustDialogAccepted = true` on a parsed `~/.claude.json`,
/// creating the `projects` map and the per-project entry as needed and leaving
/// all other content untouched. Pure, so it unit-tests without the filesystem.
pub(super) fn with_workspace_trusted(mut root: serde_json::Value, key: &str) -> serde_json::Value {
    use serde_json::{json, Value};
    if let Some(obj) = root.as_object_mut() {
        let projects = obj.entry("projects").or_insert_with(|| json!({}));
        if let Some(projects) = projects.as_object_mut() {
            let entry = projects.entry(key.to_string()).or_insert_with(|| json!({}));
            if let Some(entry) = entry.as_object_mut() {
                entry.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
            }
        }
    }
    root
}

/// Mark Claude Code's first-run onboarding wizard complete on a parsed
/// `~/.claude.json`, so an interactive session boots straight into the prompt
/// instead of the "Let's get started" / theme picker. Sets the top-level
/// `hasCompletedOnboarding` flag and seeds a `theme` only when one is absent (so
/// a user's chosen theme is never overwritten). Leaves all other content intact.
/// Pure, so it unit-tests without the filesystem.
pub(super) fn with_onboarding_completed(mut root: serde_json::Value) -> serde_json::Value {
    use serde_json::{json, Value};
    if let Some(obj) = root.as_object_mut() {
        obj.insert("hasCompletedOnboarding".into(), Value::Bool(true));
        obj.entry("theme").or_insert_with(|| json!("dark"));
    }
    root
}
