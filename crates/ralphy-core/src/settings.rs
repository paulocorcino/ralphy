//! Per-repo operator configuration (ADR-0010).
//!
//! [`Settings`] is persisted to `<repo>/.ralphy/settings.json`. Unknown keys
//! are tolerated and round-tripped via `#[serde(flatten)]` so an older binary
//! never silently drops a key written by a newer one.
//!
//! The store holds only agent-agnostic keys. Each adapter's settings live in a
//! top-level section named after the adapter, kept as raw JSON in `extra` and
//! (de)serialized by the adapter's own typed struct through
//! [`Settings::agent_settings`] / [`Settings::set_agent_settings`] — the core
//! never interprets a section's contents (ADR-0002 amendment, #79).

use std::fs;
use std::io::ErrorKind;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Map;

use crate::Workspace;

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
    /// When `true`, an issue whose verify resolution lands on no gate at all
    /// (no `## Verify` in the plan AND no `verify.command` fallback) is NOT
    /// closed on the agent's self-report: it is labeled `ready-for-human` and
    /// left open, and the run continues (ADR-0015). `None`/`false` keeps the
    /// ADR-0011 behavior — close with a loud warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_verify_gate: Option<bool>,
}

/// The full settings store. Fields are additive across releases; unknown keys
/// are preserved by the `extra` flatten so an older binary's `save` does not
/// silently drop a future peer's keys. Per-agent sections (e.g. a vendor's
/// model/effort defaults) also live in `extra` — see [`Settings::agent_settings`].
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Agent-agnostic base branch default (`--base-branch`). `None` → hardcoded
    /// `origin/main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    /// Agent-agnostic branch-mode default (`--branch-mode`), stored as the
    /// lowercase canonical string `"new"`/`"current"`. `None` → hardcoded `new`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_mode: Option<String>,
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

    /// Deserialize one agent's settings section — a top-level key in the
    /// settings file named after the adapter — into the adapter's own typed
    /// struct. An absent section yields `T::default()`. Unknown keys *inside*
    /// a section are ignored on load (as they were when the sections were
    /// typed fields here), so a typed re-save preserves today's behavior.
    pub fn agent_settings<T: serde::de::DeserializeOwned + Default>(
        &self,
        section: &str,
    ) -> Result<T> {
        match self.extra.get(section) {
            Some(v) => serde_json::from_value(v.clone())
                .with_context(|| format!("parsing settings section '{section}'")),
            None => Ok(T::default()),
        }
    }

    /// Serialize an adapter's typed settings struct back into its section.
    /// The section key is written even when the struct is all-default; callers
    /// that want an absent key should remove it from `extra` instead.
    pub fn set_agent_settings<T: Serialize>(&mut self, section: &str, value: &T) -> Result<()> {
        let v = serde_json::to_value(value)
            .with_context(|| format!("serializing settings section '{section}'"))?;
        self.extra.insert(section.to_owned(), v);
        Ok(())
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

    /// A stand-in for an adapter's typed settings slice; the core only ever
    /// sees the section as raw JSON.
    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct FakeAgentSettings {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    }

    #[test]
    fn round_trip_set_and_unset() {
        let (ws, dir) = tmp_ws("round-trip");

        // Default load on missing file: no section, so the typed view defaults.
        let s = Settings::load(&ws).unwrap();
        let a: FakeAgentSettings = s.agent_settings("agent_x").unwrap();
        assert_eq!(a.model, None);

        // Set a model in the section, save, reload.
        let mut s = s;
        s.set_agent_settings(
            "agent_x",
            &FakeAgentSettings {
                model: Some("model-1".into()),
            },
        )
        .unwrap();
        s.save(&ws).unwrap();
        let reloaded = Settings::load(&ws).unwrap();
        let a: FakeAgentSettings = reloaded.agent_settings("agent_x").unwrap();
        assert_eq!(a.model.as_deref(), Some("model-1"));

        // Clear the model, save, reload.
        let mut s = reloaded;
        s.set_agent_settings("agent_x", &FakeAgentSettings { model: None })
            .unwrap();
        s.save(&ws).unwrap();
        let reloaded = Settings::load(&ws).unwrap();
        let a: FakeAgentSettings = reloaded.agent_settings("agent_x").unwrap();
        assert_eq!(a.model, None);

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
        s.save(&ws).unwrap();

        let reloaded = Settings::load(&ws).unwrap();
        assert_eq!(reloaded.base_branch.as_deref(), Some("origin/dev"));
        assert_eq!(reloaded.branch_mode.as_deref(), Some("current"));

        // The unknown peer key must survive the typed save.
        let back = fs::read_to_string(ws.settings_path()).unwrap();
        assert!(
            back.contains("future_key"),
            "flatten must preserve unknown keys; got:\n{back}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn require_verify_gate_round_trips_and_defaults_unset() {
        let (ws, dir) = tmp_ws("require-gate");

        // Out of the box: unset (the runner treats that as `false`).
        let s = Settings::load(&ws).unwrap();
        assert_eq!(s.verify.require_verify_gate, None);

        let mut s = s;
        s.verify.require_verify_gate = Some(true);
        s.save(&ws).unwrap();
        let reloaded = Settings::load(&ws).unwrap();
        assert_eq!(reloaded.verify.require_verify_gate, Some(true));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_key_tolerance() {
        let (ws, dir) = tmp_ws("unknown-key");
        // Write a settings file containing an agent section and an unknown peer key.
        let raw = r#"{"agent_x":{"model":"x"},"future_key":123}"#;
        fs::create_dir_all(ws.ralphy_dir()).unwrap();
        fs::write(ws.settings_path(), raw).unwrap();

        // Load succeeds and the typed view surfaces the section's model.
        let s = Settings::load(&ws).unwrap();
        let a: FakeAgentSettings = s.agent_settings("agent_x").unwrap();
        assert_eq!(a.model.as_deref(), Some("x"));

        // Save and re-read the raw file — `future_key` must survive.
        s.save(&ws).unwrap();
        let back = fs::read_to_string(ws.settings_path()).unwrap();
        assert!(
            back.contains("future_key"),
            "flatten must preserve unknown keys; got:\n{back}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn agent_section_round_trips_through_save() {
        let (ws, dir) = tmp_ws("agent-section");

        let mut s = Settings::default();
        s.set_agent_settings(
            "agent_x",
            &FakeAgentSettings {
                model: Some("model-2".into()),
            },
        )
        .unwrap();
        s.save(&ws).unwrap();

        let reloaded = Settings::load(&ws).unwrap();
        let a: FakeAgentSettings = reloaded.agent_settings("agent_x").unwrap();
        assert_eq!(a.model.as_deref(), Some("model-2"));

        // A malformed section is a section-level error, not a load failure.
        let raw = r#"{"agent_x":"not-an-object"}"#;
        fs::write(ws.settings_path(), raw).unwrap();
        let s = Settings::load(&ws).unwrap();
        let err = s.agent_settings::<FakeAgentSettings>("agent_x").unwrap_err();
        assert!(err.to_string().contains("agent_x"), "got: {err}");

        fs::remove_dir_all(&dir).ok();
    }
}
