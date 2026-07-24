//! Cursor-specific settings persisted under the [`CursorSettings::SECTION`]
//! section of `.ralphy/settings.json` (ADR-0010). The core stores the section as
//! opaque JSON; this adapter owns the schema (ADR-0002 amendment, #79).

/// The one persisted key `--agent cursor` carries in this slice (ADR-0042 D6).
/// Per-phase model overrides come from `--plan-model`/`--exec-model`, not from
/// persisted keys — Cursor's model axis is an entitlement, and D4 forbids
/// omitting `--model`, so there is no "unset" state worth persisting.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CursorSettings {
    /// D6's escape hatch: `true` lets a run proceed in a repository with no
    /// `.cursorindexingignore`, i.e. one whose contents the vendor will walk and
    /// sync to its servers. The name is deliberately verbose — length is the
    /// safety feature, so it cannot be set by accident.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_codebase_indexing_i_understand_the_risk: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl CursorSettings {
    /// The settings-file section this struct lives under.
    pub const SECTION: &'static str = "cursor";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_settings_defaults_are_false() {
        let d = CursorSettings::default();
        assert!(
            !d.allow_codebase_indexing_i_understand_the_risk,
            "D6's hatch is off unless the operator sets it"
        );
        // An untouched section must not start serializing into every settings file.
        assert_eq!(
            serde_json::to_string(&CursorSettings::default()).unwrap(),
            "{}"
        );
    }

    #[test]
    fn cursor_settings_round_trips_json() {
        let s: CursorSettings =
            serde_json::from_str(r#"{"allow_codebase_indexing_i_understand_the_risk":true}"#)
                .unwrap();
        assert!(s.allow_codebase_indexing_i_understand_the_risk);
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"allow_codebase_indexing_i_understand_the_risk":true}"#
        );
        // An empty section parses to the safe default.
        let empty: CursorSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, CursorSettings::default());
    }
}
