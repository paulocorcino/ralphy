//! The agent sections of the settings: Claude, Copilot, Cursor and Antigravity keys.

use super::*;
use ralphy_agent_antigravity::AntigravitySettings;
use ralphy_agent_copilot::CopilotSettings;
use ralphy_agent_cursor::CursorSettings;

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

/// A `gemini` section left in settings.json by an older version is ignored, with one
/// warning that names the keys to use now.
#[test]
fn a_stored_gemini_section_prints_one_warning_naming_the_antigravity_keys() {
    let (ws, dir) = tmp_ws("gemini-section-warning");
    let clean = Settings::load(&ws).unwrap();
    assert_eq!(view::stored_gemini_warning(&clean), None);

    fs::create_dir_all(ws.ralphy_dir()).unwrap();
    fs::write(
        ws.settings_path(),
        r#"{"gemini":{"plan_model":"gemini-2.5-pro"}}"#,
    )
    .unwrap();
    let s = Settings::load(&ws).unwrap();
    let warning = view::stored_gemini_warning(&s).expect("a stored section warns");
    assert!(warning.contains("antigravity.plan_model"), "{warning}");
    assert!(warning.contains("antigravity.exec_model"), "{warning}");
    assert_eq!(warning.lines().count(), 1, "{warning}");

    remove_scratch(&dir);
}

#[test]
fn config_unset_still_removes_a_gemini_key() {
    let (ws, dir) = tmp_ws("gemini-unset");
    fs::create_dir_all(ws.ralphy_dir()).unwrap();
    fs::write(
        ws.settings_path(),
        r#"{"gemini":{"plan_model":"gemini-2.5-pro","exec_model":"gemini-3.5-flash"}}"#,
    )
    .unwrap();

    unset(&ws, "gemini.plan_model").unwrap();
    let s = Settings::load(&ws).unwrap();
    let section = s.extra.get("gemini").expect("the other key stays stored");
    assert_eq!(section.get("plan_model"), None);
    assert!(section.get("exec_model").is_some());

    unset(&ws, "gemini.exec_model").unwrap();
    let s = Settings::load(&ws).unwrap();
    assert_eq!(view::stored_gemini_warning(&s), None);

    remove_scratch(&dir);
}

#[test]
fn config_set_of_a_gemini_key_is_an_unknown_key() {
    let (ws, dir) = tmp_ws("gemini-set-refused");
    for key in ["gemini.plan_model", "gemini.exec_model"] {
        let err = set(&ws, key, "gemini-3.5-flash").expect_err("set must refuse");
        assert!(err.to_string().contains("unknown config key"), "{err}");
    }
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

fn agy_settings(ws: &Workspace) -> AntigravitySettings {
    let s = Settings::load(ws).unwrap();
    s.agent_settings(AntigravitySettings::SECTION).unwrap()
}

#[test]
fn antigravity_model_keys_round_trip() {
    let (ws, dir) = tmp_ws("antigravity-model-round-trip");

    set(&ws, "antigravity.plan_model", "gemini-3.8-pro").unwrap();
    set(&ws, "antigravity.exec_model", "gemini-3.8-flash-low").unwrap();
    let a = agy_settings(&ws);
    assert_eq!(a.plan_model.as_deref(), Some("gemini-3.8-pro"));
    assert_eq!(a.exec_model.as_deref(), Some("gemini-3.8-flash-low"));
    let j = config_json(&ws).unwrap();
    assert_eq!(
        j["antigravity.plan_model"],
        serde_json::json!("gemini-3.8-pro")
    );
    get(&ws, false).unwrap();

    unset(&ws, "antigravity.plan_model").unwrap();
    unset(&ws, "antigravity.exec_model").unwrap();
    let a = agy_settings(&ws);
    assert_eq!((a.plan_model, a.exec_model), (None, None));
    let j = config_json(&ws).unwrap();
    assert_eq!(j["antigravity.exec_model"], serde_json::Value::Null);

    remove_scratch(&dir);
}

#[test]
fn antigravity_effort_keys_accept_only_the_core_lexicon() {
    for (index, value) in ["low", "medium", "high", "xhigh", "max"]
        .into_iter()
        .enumerate()
    {
        let (ws, dir) = tmp_ws(&format!("antigravity-effort-{index}"));
        set(&ws, "antigravity.plan_effort", value).unwrap();
        set(&ws, "antigravity.exec_effort", value).unwrap();
        let a = agy_settings(&ws);
        assert_eq!(a.plan_effort.as_deref(), Some(value));
        assert_eq!(a.exec_effort.as_deref(), Some(value));
        remove_scratch(dir);
    }

    for invalid in ["none", "minimal", "extreme"] {
        for key in ["antigravity.plan_effort", "antigravity.exec_effort"] {
            let (ws, dir) = tmp_ws(&format!("{key}-{invalid}"));
            assert!(set(&ws, key, invalid).is_err());
            let a = agy_settings(&ws);
            assert_eq!((a.plan_effort, a.exec_effort), (None, None));
            remove_scratch(dir);
        }
    }
}

#[test]
fn antigravity_waiver_is_set_only_on_this_machine() {
    const KEY: &str = "antigravity.run_without_guard_proof_i_understand_the_risk";
    let (ws, dir) = tmp_ws("antigravity-waiver");
    assert!(!agy_settings(&ws).run_without_guard_proof_i_understand_the_risk);

    set(&ws, KEY, "true").unwrap();
    let raw = fs::read_to_string(ws.settings_path()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        v["antigravity"]["run_without_guard_proof_i_understand_the_risk"],
        serde_json::json!(true)
    );
    assert_eq!(config_json(&ws).unwrap()[KEY], serde_json::json!(true));

    let err = set(&ws, KEY, "yes").expect_err("only 'true'/'false' are accepted");
    assert!(err.to_string().contains("'true' or 'false'"), "{err}");
    assert!(agy_settings(&ws).run_without_guard_proof_i_understand_the_risk);

    unset(&ws, KEY).unwrap();
    assert!(!agy_settings(&ws).run_without_guard_proof_i_understand_the_risk);
    assert_eq!(config_json(&ws).unwrap()[KEY], serde_json::json!(false));

    remove_scratch(&dir);
}

#[test]
fn an_antigravity_model_with_a_space_or_empty_is_refused() {
    let (ws, dir) = tmp_ws("antigravity-model-shape");
    for bad in ["", "   ", "gemini 3.8", "Gemini-3.8", "gemini/3.8"] {
        assert!(set(&ws, "antigravity.plan_model", bad).is_err(), "{bad:?}");
        assert!(set(&ws, "antigravity.exec_model", bad).is_err(), "{bad:?}");
    }
    let a = agy_settings(&ws);
    assert_eq!((a.plan_model, a.exec_model), (None, None));
    remove_scratch(&dir);
}

#[test]
fn config_json_prints_the_antigravity_keys() {
    let (ws, dir) = tmp_ws("antigravity-config-json");
    let j = config_json(&ws).unwrap();
    for key in [
        "antigravity.plan_model",
        "antigravity.exec_model",
        "antigravity.plan_effort",
        "antigravity.exec_effort",
        "antigravity.run_without_guard_proof_i_understand_the_risk",
    ] {
        assert!(j.get(key).is_some(), "{key} missing from config --json");
    }
    remove_scratch(&dir);
}

#[test]
fn an_antigravity_model_that_starts_with_a_dash_is_refused() {
    let (ws, dir) = tmp_ws("antigravity-model-dash");
    for bad in ["-x", "--sandbox", ".gemini"] {
        assert!(set(&ws, "antigravity.plan_model", bad).is_err(), "{bad:?}");
        assert!(set(&ws, "antigravity.exec_model", bad).is_err(), "{bad:?}");
    }
    set(&ws, "antigravity.plan_model", "3-flash").unwrap();
    assert_eq!(agy_settings(&ws).plan_model.as_deref(), Some("3-flash"));
    remove_scratch(&dir);
}
