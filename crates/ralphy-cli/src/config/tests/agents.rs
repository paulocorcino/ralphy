//! The agent sections of the settings: Claude, Copilot, Cursor and Gemini keys.

use super::*;
use ralphy_agent_copilot::CopilotSettings;
use ralphy_agent_cursor::CursorSettings;
use ralphy_agent_gemini::GeminiSettings;

#[test]
fn copilot_config_round_trip() {
    let (ws, dir) = tmp_ws("copilot-config-round-trip");

    set(&ws, "copilot.plan_model", "gpt-5").unwrap();
    let s = Settings::load(&ws).unwrap();
    let c: CopilotSettings = s.agent_settings(CopilotSettings::SECTION).unwrap();
    assert_eq!(c.plan_model.as_deref(), Some("gpt-5"));
    assert_eq!(c.exec_model, None);

    set(&ws, "copilot.exec_model", "claude-sonnet-5").unwrap();
    let s = Settings::load(&ws).unwrap();
    let c: CopilotSettings = s.agent_settings(CopilotSettings::SECTION).unwrap();
    assert_eq!(c.plan_model.as_deref(), Some("gpt-5"));
    assert_eq!(c.exec_model.as_deref(), Some("claude-sonnet-5"));

    unset(&ws, "copilot.plan_model").unwrap();
    unset(&ws, "copilot.exec_model").unwrap();
    let s = Settings::load(&ws).unwrap();
    let c: CopilotSettings = s.agent_settings(CopilotSettings::SECTION).unwrap();
    assert_eq!(c.plan_model, None);
    assert_eq!(c.exec_model, None);

    remove_scratch(&dir);
}

#[test]
fn claude_effort_config_accepts_only_core_values() {
    for (index, value) in ["low", "medium", "high", "xhigh", "max"]
        .into_iter()
        .enumerate()
    {
        let (ws, dir) = tmp_ws(&format!("claude-effort-{index}"));
        set(&ws, "claude.plan_effort", value).unwrap();
        set(&ws, "claude.exec_effort", value).unwrap();
        let settings = Settings::load(&ws).unwrap();
        let claude: ClaudeSettings = settings.agent_settings(ClaudeSettings::SECTION).unwrap();
        assert_eq!(claude.plan_effort.as_deref(), Some(value));
        assert_eq!(claude.exec_effort.as_deref(), Some(value));
        remove_scratch(dir);
    }

    for invalid in ["none", "minimal", "extreme"] {
        for key in ["claude.plan_effort", "claude.exec_effort"] {
            let (ws, dir) = tmp_ws(&format!("{key}-{invalid}"));
            assert!(set(&ws, key, invalid).is_err());
            let settings = Settings::load(&ws).unwrap();
            let claude: ClaudeSettings = settings.agent_settings(ClaudeSettings::SECTION).unwrap();
            assert_eq!(claude.plan_effort, None);
            assert_eq!(claude.exec_effort, None);
            remove_scratch(dir);
        }
    }
}

/// ADR-0042 D6's escape hatch (#243): the one Cursor key. Same bool discipline
/// as Copilot's hatch, guarding a bigger capability — `true` lets a run proceed
/// in a repository whose contents the vendor will walk and sync to its servers.
#[test]
fn cursor_config_round_trip() {
    const KEY: &str = "cursor.allow_codebase_indexing_i_understand_the_risk";
    let (ws, dir) = tmp_ws("cursor-config-round-trip");

    let s = Settings::load(&ws).unwrap();
    let c: CursorSettings = s.agent_settings(CursorSettings::SECTION).unwrap();
    assert!(
        !c.allow_codebase_indexing_i_understand_the_risk,
        "the hatch is off until the operator sets it"
    );

    set(&ws, KEY, "true").unwrap();
    let s = Settings::load(&ws).unwrap();
    let c: CursorSettings = s.agent_settings(CursorSettings::SECTION).unwrap();
    assert!(
        c.allow_codebase_indexing_i_understand_the_risk,
        "the opt-in must persist and reload"
    );
    assert_eq!(config_json(&ws).unwrap()[KEY], serde_json::json!(true));

    let err = set(&ws, KEY, "yes").expect_err("only 'true'/'false' are accepted");
    assert!(err.to_string().contains("'true' or 'false'"), "{err}");
    let s = Settings::load(&ws).unwrap();
    let c: CursorSettings = s.agent_settings(CursorSettings::SECTION).unwrap();
    assert!(
        c.allow_codebase_indexing_i_understand_the_risk,
        "the refused write must leave the stored value alone"
    );

    unset(&ws, KEY).unwrap();
    let s = Settings::load(&ws).unwrap();
    let c: CursorSettings = s.agent_settings(CursorSettings::SECTION).unwrap();
    assert!(!c.allow_codebase_indexing_i_understand_the_risk);

    remove_scratch(&dir);
}

/// ADR-0043 D8 (#257): the two Gemini per-phase pins persist, print, emit and
/// clear — and an id the CLI no longer serves is refused HERE, before a spawn.
#[test]
fn gemini_config_round_trip() {
    let (ws, dir) = tmp_ws("gemini-config-round-trip");

    set(&ws, "gemini.plan_model", "gemini-2.5-pro").unwrap();
    set(&ws, "gemini.exec_model", "gemini-3.5-flash").unwrap();
    let s = Settings::load(&ws).unwrap();
    let g: GeminiSettings = s.agent_settings(GeminiSettings::SECTION).unwrap();
    assert_eq!(g.plan_model.as_deref(), Some("gemini-2.5-pro"));
    assert_eq!(g.exec_model.as_deref(), Some("gemini-3.5-flash"));

    let json = config_json(&ws).unwrap();
    assert_eq!(
        json["gemini.exec_model"],
        serde_json::json!("gemini-3.5-flash")
    );
    assert_eq!(
        json["gemini.plan_model"],
        serde_json::json!("gemini-2.5-pro")
    );
    get(&ws, false).unwrap();

    // A padded value is trimmed on the way in, not stored raw to 404 later.
    set(&ws, "gemini.exec_model", "  gemini-2.5-flash  ").unwrap();
    let s = Settings::load(&ws).unwrap();
    let g: GeminiSettings = s.agent_settings(GeminiSettings::SECTION).unwrap();
    assert_eq!(g.exec_model.as_deref(), Some("gemini-2.5-flash"));
    set(&ws, "gemini.exec_model", "gemini-3.5-flash").unwrap();

    // The retired id is refused at configuration time, naming what was asked.
    let err = set(&ws, "gemini.exec_model", "gemini-3-pro-preview")
        .expect_err("a retired id must not be pinnable");
    assert!(err.to_string().contains("gemini-3-pro-preview"), "{err}");
    let s = Settings::load(&ws).unwrap();
    let g: GeminiSettings = s.agent_settings(GeminiSettings::SECTION).unwrap();
    assert_eq!(
        g.exec_model.as_deref(),
        Some("gemini-3.5-flash"),
        "the refused write must leave the stored value alone"
    );

    unset(&ws, "gemini.plan_model").unwrap();
    unset(&ws, "gemini.exec_model").unwrap();
    let s = Settings::load(&ws).unwrap();
    let g: GeminiSettings = s.agent_settings(GeminiSettings::SECTION).unwrap();
    assert_eq!(g.plan_model, None);
    assert_eq!(g.exec_model, None);

    remove_scratch(&dir);
}

/// D7's escape hatch (#234): a bool key that only `'true'`/`'false'` set, so a
/// hopeful `yes` cannot silently hand Copilot back its credentialled MCP server.
#[test]
fn copilot_allow_builtin_mcps_round_trip() {
    const KEY: &str = "copilot.allow_builtin_mcp_servers_i_understand_the_risk";
    let (ws, dir) = tmp_ws("copilot-allow-builtin-mcps");

    let s = Settings::load(&ws).unwrap();
    let c: CopilotSettings = s.agent_settings(CopilotSettings::SECTION).unwrap();
    assert!(
        !c.allow_builtin_mcp_servers_i_understand_the_risk,
        "the hatch is off until the operator sets it"
    );

    set(&ws, KEY, "true").unwrap();
    let s = Settings::load(&ws).unwrap();
    let c: CopilotSettings = s.agent_settings(CopilotSettings::SECTION).unwrap();
    assert!(c.allow_builtin_mcp_servers_i_understand_the_risk);
    assert_eq!(config_json(&ws).unwrap()[KEY], serde_json::json!(true));

    let err = set(&ws, KEY, "yes").expect_err("only 'true'/'false' are accepted");
    assert!(err.to_string().contains("'true' or 'false'"), "{err}");
    // The refused write left the stored value alone.
    let s = Settings::load(&ws).unwrap();
    let c: CopilotSettings = s.agent_settings(CopilotSettings::SECTION).unwrap();
    assert!(c.allow_builtin_mcp_servers_i_understand_the_risk);

    unset(&ws, KEY).unwrap();
    let s = Settings::load(&ws).unwrap();
    let c: CopilotSettings = s.agent_settings(CopilotSettings::SECTION).unwrap();
    assert!(!c.allow_builtin_mcp_servers_i_understand_the_risk);
    assert_eq!(config_json(&ws).unwrap()[KEY], serde_json::json!(false));

    remove_scratch(&dir);
}

/// The two effort keys land in the same `copilot` section and surface through
/// `config_json` — the shape the daemon's `config` verb reads.
#[test]
fn copilot_effort_config_round_trip() {
    let (ws, dir) = tmp_ws("copilot-effort-round-trip");

    set(&ws, "copilot.plan_effort", "high").unwrap();
    set(&ws, "copilot.exec_effort", "low").unwrap();
    let s = Settings::load(&ws).unwrap();
    let c: CopilotSettings = s.agent_settings(CopilotSettings::SECTION).unwrap();
    assert_eq!(c.plan_effort.as_deref(), Some("high"));
    assert_eq!(c.exec_effort.as_deref(), Some("low"));
    let j = config_json(&ws).unwrap();
    assert_eq!(j["copilot.plan_effort"], serde_json::json!("high"));
    assert_eq!(j["copilot.exec_effort"], serde_json::json!("low"));

    unset(&ws, "copilot.plan_effort").unwrap();
    unset(&ws, "copilot.exec_effort").unwrap();
    let s = Settings::load(&ws).unwrap();
    let c: CopilotSettings = s.agent_settings(CopilotSettings::SECTION).unwrap();
    assert_eq!(c.plan_effort, None);
    assert_eq!(c.exec_effort, None);
    let j = config_json(&ws).unwrap();
    assert_eq!(j["copilot.plan_effort"], serde_json::Value::Null);

    // A typo is refused at `set` time: an unrankable level is silently
    // dropped by the adapter's clamp, so persisting it would leave the
    // operator with a setting `config get` reports as set and that does
    // nothing.
    let err = set(&ws, "copilot.plan_effort", "hgih").expect_err("a typo must be refused");
    assert!(
        err.to_string().contains("reasoning-effort level"),
        "error: {err}"
    );
    assert_eq!(
        config_json(&ws).unwrap()["copilot.plan_effort"],
        serde_json::Value::Null,
        "a refused set must persist nothing"
    );

    remove_scratch(&dir);
}
