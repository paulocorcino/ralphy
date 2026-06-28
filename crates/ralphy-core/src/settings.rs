//! Per-repo operator configuration (ADR-0010).
//!
//! [`Settings`] is persisted to `<repo>/.ralphy/settings.json`. Unknown keys
//! are tolerated and round-tripped via `#[serde(flatten)]` so an older binary
//! never silently drops a key written by a newer one.

use std::fs;
use std::io::ErrorKind;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Map;

use crate::Workspace;

/// OpenCode-specific settings.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenCodeSettings {
    /// The model id to pass as `-m <id>` when no `--exec-model` flag is given.
    /// `None` / empty → OpenCode resolves the model itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Claude-specific run defaults (ADR-0010). These knobs are consumed only by the
/// Claude adapter in the current wiring, so they are namespaced under `claude.*`;
/// a Codex peer can be added later as a sibling struct without migration. Each
/// field is `None` out of the box, leaving the hardcoded run defaults in place.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeSettings {
    /// Planning model default (`--plan-model`). `None` → hardcoded `opus`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_model: Option<String>,
    /// Planning effort default (`--plan-effort`). `None` → hardcoded `medium`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_effort: Option<String>,
    /// Execution model used when the plan emits no complexity judgment
    /// (`--default-exec-model`). `None` → hardcoded `sonnet`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_exec_model: Option<String>,
    /// Execution effort default (`--exec-effort`). `None` → hardcoded `medium`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_effort: Option<String>,
    /// Per-issue wall-clock budget in minutes (`--max-minutes-per-issue`).
    /// `None` → [`crate::DEFAULT_MAX_MINUTES_PER_ISSUE`]; `Some(0)` disables the
    /// per-issue cap (the issue is then bounded only by `--deadline-hours`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_minutes_per_issue: Option<u64>,
}

/// The per-repo verify-gate default (ADR-0011). `command` is the fallback verify
/// command used when a plan's `## Verify` section is absent or empty — a single
/// command line, tokenized into argv (no shell). `None` leaves the gate
/// unconfigured, so an unspecified plan closes on the self-report with a loud warn.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifySettings {
    /// The fallback verify command (`verify.command`). One command line; the
    /// runner tokenizes it into argv and runs it directly. `None` → no fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// The full settings store. Fields are additive across releases; unknown keys
/// are preserved by the `extra` flatten so an older binary's `save` does not
/// silently drop a future peer's keys.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub opencode: OpenCodeSettings,
    /// Agent-agnostic base branch default (`--base-branch`). `None` → hardcoded
    /// `origin/main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    /// Agent-agnostic branch-mode default (`--branch-mode`), stored as the
    /// lowercase canonical string `"new"`/`"current"`. `None` → hardcoded `new`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_mode: Option<String>,
    #[serde(default)]
    pub claude: ClaudeSettings,
    /// The runner-enforced verify gate's per-repo fallback (ADR-0011).
    #[serde(default)]
    pub verify: VerifySettings,
    #[serde(flatten)]
    pub extra: Map<String, serde_json::Value>,
}

impl Settings {
    /// Load settings from `<repo>/.ralphy/settings.json`.
    /// Returns [`Settings::default`] when the file is absent (out-of-the-box state).
    pub fn load(ws: &Workspace) -> Result<Settings> {
        let path = ws.settings_path();
        match fs::read_to_string(&path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Write settings to `<repo>/.ralphy/settings.json`, creating the
    /// `.ralphy/` directory if needed.
    pub fn save(&self, ws: &Workspace) -> Result<()> {
        let dir = ws.ralphy_dir();
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = ws.settings_path();
        let text = serde_json::to_string_pretty(self).context("serializing settings")?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn tmp_ws(name: &str) -> (Workspace, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "ralphy-settings-{}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed),
            name
        ));
        fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::new(&dir);
        (ws, dir)
    }

    #[test]
    fn round_trip_set_and_unset() {
        let (ws, dir) = tmp_ws("round-trip");

        // Default load on missing file.
        let s = Settings::load(&ws).unwrap();
        assert_eq!(s.opencode.model, None);

        // Set a model, save, reload.
        let mut s = s;
        s.opencode.model = Some("kimi-for-coding/k2p7".into());
        s.save(&ws).unwrap();
        let reloaded = Settings::load(&ws).unwrap();
        assert_eq!(
            reloaded.opencode.model.as_deref(),
            Some("kimi-for-coding/k2p7")
        );

        // Unset the model, save, reload.
        let mut s = reloaded;
        s.opencode.model = None;
        s.save(&ws).unwrap();
        let reloaded = Settings::load(&ws).unwrap();
        assert_eq!(reloaded.opencode.model, None);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn new_keys_round_trip() {
        let (ws, dir) = tmp_ws("new-keys-round-trip");

        // Seed a raw file carrying an unknown peer key so we can assert it
        // survives a save that also writes the new typed keys.
        let raw = r#"{"future_key":123}"#;
        fs::create_dir_all(ws.ralphy_dir()).unwrap();
        fs::write(ws.settings_path(), raw).unwrap();

        let mut s = Settings::load(&ws).unwrap();
        s.base_branch = Some("origin/dev".into());
        s.branch_mode = Some("current".into());
        s.claude.max_minutes_per_issue = Some(45);
        s.claude.plan_model = Some("opus".into());
        s.save(&ws).unwrap();

        let reloaded = Settings::load(&ws).unwrap();
        assert_eq!(reloaded.base_branch.as_deref(), Some("origin/dev"));
        assert_eq!(reloaded.branch_mode.as_deref(), Some("current"));
        assert_eq!(reloaded.claude.max_minutes_per_issue, Some(45));
        assert_eq!(reloaded.claude.plan_model.as_deref(), Some("opus"));

        // The unknown peer key must survive the typed save.
        let back = fs::read_to_string(ws.settings_path()).unwrap();
        assert!(
            back.contains("future_key"),
            "flatten must preserve unknown keys; got:\n{back}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_key_tolerance() {
        let (ws, dir) = tmp_ws("unknown-key");
        // Write a settings file containing an unknown peer key.
        let raw = r#"{"opencode":{"model":"x"},"future_key":123}"#;
        fs::create_dir_all(ws.ralphy_dir()).unwrap();
        fs::write(ws.settings_path(), raw).unwrap();

        // Load succeeds and surfaces the opencode model.
        let s = Settings::load(&ws).unwrap();
        assert_eq!(s.opencode.model.as_deref(), Some("x"));

        // Save and re-read the raw file — `future_key` must survive.
        s.save(&ws).unwrap();
        let back = fs::read_to_string(ws.settings_path()).unwrap();
        assert!(
            back.contains("future_key"),
            "flatten must preserve unknown keys; got:\n{back}"
        );

        fs::remove_dir_all(&dir).ok();
    }
}
