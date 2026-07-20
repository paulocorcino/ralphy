//! Copilot-specific settings persisted under the [`CopilotSettings::SECTION`]
//! section of `.ralphy/settings.json` (ADR-0010). The core stores the section as
//! opaque JSON; this adapter owns the schema (ADR-0002 amendment, #79).

/// Per-phase model overrides persisted for `--agent copilot` (ADR-0041 D4).
/// `None` on either field omits `--model` for that phase, which selects the
/// account's own current default rather than a degraded fallback.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CopilotSettings {
    /// The model id to pass as `--model <id>` during `plan()` when no
    /// `--plan-model` flag is given. `None` → omit `--model` for that phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_model: Option<String>,
    /// The model id to pass as `--model <id>` during `execute()` when no
    /// `--exec-model` flag is given. `None` → omit `--model` for that phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_model: Option<String>,
    /// The reasoning effort requested for `plan()`. CLAMPED per model before it
    /// reaches argv (ADR-0041 D5a): a level the phase's model does not publish is
    /// lowered, and a model that takes no effort argument never receives the flag.
    /// `None` → omit `--effort` for that phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_effort: Option<String>,
    /// The reasoning effort requested for `execute()`. Same clamp; `None` → omit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_effort: Option<String>,
}

impl CopilotSettings {
    /// The settings-file section this struct lives under.
    pub const SECTION: &'static str = "copilot";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_settings_defaults_are_all_none() {
        let d = CopilotSettings::default();
        assert_eq!(d.plan_model, None);
        assert_eq!(d.exec_model, None);
        assert_eq!(d.plan_effort, None);
        assert_eq!(d.exec_effort, None);
    }

    #[test]
    fn copilot_settings_round_trips_json() {
        let s: CopilotSettings =
            serde_json::from_str(r#"{"plan_model":"a","exec_model":"b"}"#).unwrap();
        assert_eq!(s.plan_model.as_deref(), Some("a"));
        assert_eq!(s.exec_model.as_deref(), Some("b"));
        let e: CopilotSettings =
            serde_json::from_str(r#"{"plan_effort":"high","exec_effort":"low"}"#).unwrap();
        assert_eq!(e.plan_effort.as_deref(), Some("high"));
        assert_eq!(e.exec_effort.as_deref(), Some("low"));
        assert_eq!(
            e.plan_model, None,
            "an effort-only section leaves models unset"
        );
        assert_eq!(
            serde_json::to_string(&CopilotSettings::default()).unwrap(),
            "{}"
        );
    }

    // Fragments are split with `concat!` so this assertion doesn't match ITSELF
    // via `include_str!` (the whole-file self-scan trap).
    #[test]
    fn copilot_source_hardcodes_no_model_id() {
        let src = include_str!("settings.rs");
        assert!(!src.contains(concat!("claude", "-sonnet")));
        assert!(!src.contains(concat!("gpt", "-5")));
    }
}
