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
    /// The D7 escape hatch (ADR-0041): `true` drops `--disable-builtin-mcps` from
    /// the argv AND suppresses the connected-builtin-server failure, handing the
    /// operator back Copilot's bundled GitHub MCP server — which holds their
    /// GitHub credential and can open a PR without `git push`. The name is
    /// deliberately verbose: length is the safety feature, so it cannot be set by
    /// accident.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_builtin_mcp_servers_i_understand_the_risk: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl CopilotSettings {
    /// The settings-file section this struct lives under.
    pub const SECTION: &'static str = "copilot";
}

#[cfg(test)]
mod tests {
    // Fragments are split with `concat!` so this assertion doesn't match ITSELF
    // via `include_str!` (the whole-file self-scan trap).
    #[test]
    fn copilot_source_hardcodes_no_model_id() {
        let src = include_str!("settings.rs");
        assert!(!src.contains(concat!("claude", "-sonnet")));
        assert!(!src.contains(concat!("gpt", "-5")));
    }
}
