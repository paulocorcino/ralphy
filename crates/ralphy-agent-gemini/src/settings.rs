//! Gemini-specific settings persisted under the [`GeminiSettings::SECTION`]
//! section of `.ralphy/settings.json` (ADR-0010). The core stores the section as
//! opaque JSON; this adapter owns the schema (ADR-0002 amendment, #79).

/// The per-phase model pins `--agent gemini` carries (ADR-0043 D8). `None` on
/// either omits `-m` for that phase, which on this vendor means the router picks
/// — and charges a second, paid routing call per turn.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GeminiSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_model: Option<String>,
}

impl GeminiSettings {
    /// The settings-file section this struct lives under.
    pub const SECTION: &'static str = "gemini";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_section_serializes_to_nothing() {
        // Otherwise every settings file on disk grows a `gemini` block nobody set.
        assert_eq!(
            serde_json::to_string(&GeminiSettings::default()).unwrap(),
            "{}"
        );
        let empty: GeminiSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, GeminiSettings::default());
    }

    #[test]
    fn the_two_phase_pins_round_trip() {
        let s: GeminiSettings = serde_json::from_str(
            r#"{"plan_model":"gemini-2.5-pro","exec_model":"gemini-3.5-flash"}"#,
        )
        .unwrap();
        assert_eq!(s.plan_model.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(s.exec_model.as_deref(), Some("gemini-3.5-flash"));
        let back: GeminiSettings =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
