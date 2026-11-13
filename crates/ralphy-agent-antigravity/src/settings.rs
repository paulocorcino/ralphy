//! The `antigravity` section of `.ralphy/settings.json` (ADR-0010). The core
//! stores the section as opaque JSON; this adapter owns its schema.

/// The per-phase models and efforts of `--agent antigravity` (ADR-0086 D4).
/// `None` on a model omits `--model`, and `agy` uses its own default.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AntigravitySettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_effort: Option<String>,
    /// Local-only opt-in: a session whose hooks did not prove they run goes on
    /// instead of being refused. Every hook is still written. Set only with
    /// `ralphy config set` on this machine.
    // ADR-0072 D6 and ADR-0086 D9: the escape of the proof turn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub run_without_guard_proof_i_understand_the_risk: bool,
}

impl AntigravitySettings {
    /// The settings-file section this struct lives under.
    pub const SECTION: &'static str = "antigravity";
}
