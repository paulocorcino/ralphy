use super::*;
use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};

static N: AtomicU32 = AtomicU32::new(0);

fn tmp_ws(name: &str) -> (Workspace, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "ralphy-config-{}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed),
        name
    ));
    fs::create_dir_all(&dir).unwrap();
    let ws = Workspace::new(&dir);
    (ws, dir)
}

// --- resolve_opencode_model precedence ---

#[test]
fn flag_wins_over_persisted() {
    assert_eq!(
        resolve_opencode_model(Some("flag".into()), Some("persisted".into())),
        Some("flag".into())
    );
}

#[test]
fn persisted_used_when_flag_absent() {
    assert_eq!(
        resolve_opencode_model(None, Some("kimi-for-coding/k2p7".into())),
        Some("kimi-for-coding/k2p7".into())
    );
}

#[test]
fn both_unset_returns_none() {
    assert_eq!(resolve_opencode_model(None, None), None);
}

#[test]
fn empty_flag_falls_through_to_persisted() {
    assert_eq!(
        resolve_opencode_model(Some("".into()), Some("k2p7".into())),
        Some("k2p7".into())
    );
}

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

    fs::remove_dir_all(&dir).ok();
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
        fs::remove_dir_all(dir).ok();
    }

    for invalid in ["none", "minimal", "hihg"] {
        for key in ["claude.plan_effort", "claude.exec_effort"] {
            let (ws, dir) = tmp_ws(&format!("{key}-{invalid}"));
            assert!(set(&ws, key, invalid).is_err());
            let settings = Settings::load(&ws).unwrap();
            let claude: ClaudeSettings = settings.agent_settings(ClaudeSettings::SECTION).unwrap();
            assert_eq!(claude.plan_effort, None);
            assert_eq!(claude.exec_effort, None);
            fs::remove_dir_all(dir).ok();
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

    fs::remove_dir_all(&dir).ok();
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

    fs::remove_dir_all(&dir).ok();
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

    fs::remove_dir_all(&dir).ok();
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

    fs::remove_dir_all(&dir).ok();
}

// --- resolve_str / resolve_u64 precedence ---

#[test]
fn resolve_str_flag_wins() {
    assert_eq!(
        resolve_str(Some("flag".into()), Some("persisted".into()), "default"),
        "flag"
    );
}

#[test]
fn resolve_effort_applies_precedence_and_validates_raw_settings() {
    assert_eq!(
        resolve_effort(Some(Effort::High), Some("low".into()), Some(Effort::Medium)).unwrap(),
        Some(Effort::High)
    );
    assert_eq!(
        resolve_effort(None, Some("low".into()), Some(Effort::Medium)).unwrap(),
        Some(Effort::Low)
    );
    assert_eq!(
        resolve_effort(None, None, Some(Effort::Medium)).unwrap(),
        Some(Effort::Medium)
    );
    assert_eq!(resolve_effort(None, None, None).unwrap(), None);
    assert!(resolve_effort(None, Some("hihg".into()), None).is_err());
}

#[test]
fn resolve_str_persisted_when_flag_absent_or_empty() {
    assert_eq!(
        resolve_str(None, Some("persisted".into()), "default"),
        "persisted"
    );
    assert_eq!(
        resolve_str(Some("".into()), Some("persisted".into()), "default"),
        "persisted"
    );
    // An empty persisted value also falls through to the default.
    assert_eq!(resolve_str(None, Some("".into()), "default"), "default");
}

#[test]
fn resolve_str_byte_for_byte_default() {
    // Absent flag AND absent setting yield today's hardcoded value verbatim.
    assert_eq!(resolve_str(None, None, "origin/main"), "origin/main");
}

#[test]
fn resolve_u64_flag_wins_then_persisted_then_default() {
    assert_eq!(resolve_u64(Some(10), Some(20), 90), 10);
    assert_eq!(resolve_u64(None, Some(20), 90), 20);
    assert_eq!(resolve_u64(None, None, 90), 90);
}

// --- resolve_assignee precedence ---

#[test]
fn resolve_assignee_precedence() {
    // --assignee X wins over config.
    assert_eq!(
        resolve_assignee(Some("X"), false, Some("cfg")),
        Some("X".to_string())
    );
    // --no-assignee forces None even over a set config.
    assert_eq!(resolve_assignee(None, true, Some("cfg")), None);
    // Neither flag: persisted config is used.
    assert_eq!(
        resolve_assignee(None, false, Some("cfg")),
        Some("cfg".to_string())
    );
    // Nothing anywhere: no filter.
    assert_eq!(resolve_assignee(None, false, None), None);
    // Empty strings are treated as unset: an empty flag falls through to the
    // persisted value, and an empty persisted value falls through to None.
    assert_eq!(
        resolve_assignee(Some(""), false, Some("cfg")),
        Some("cfg".to_string())
    );
    assert_eq!(resolve_assignee(None, false, Some("")), None);
}

// --- resolve_remote_control precedence ---

#[test]
fn resolve_remote_control_precedence() {
    // flag on wins over persisted off.
    assert!(resolve_remote_control(true, false, Some(false)));
    // --no wins over persisted on.
    assert!(!resolve_remote_control(false, true, Some(true)));
    // persisted on used when no flag.
    assert!(resolve_remote_control(false, false, Some(true)));
    // default OFF.
    assert!(!resolve_remote_control(false, false, None));
    // remote_control precedes no_remote_control.
    assert!(resolve_remote_control(true, true, None));
}

#[test]
fn remote_control_config_round_trip() {
    let (ws, dir) = tmp_ws("remote-control-config");

    set(&ws, "remote_control", "true").unwrap();
    let s = Settings::load(&ws).unwrap();
    assert_eq!(s.remote_control, Some(true));

    unset(&ws, "remote_control").unwrap();
    let s = Settings::load(&ws).unwrap();
    assert_eq!(s.remote_control, None);

    let err = set(&ws, "remote_control", "maybe").unwrap_err();
    assert!(
        err.to_string().contains("must be 'true' or 'false'"),
        "got: {err}"
    );

    fs::remove_dir_all(&dir).ok();
}

// --- parse_branch_mode ---

#[test]
fn parse_branch_mode_ok_arms() {
    assert_eq!(parse_branch_mode("new").unwrap(), BranchMode::New);
    assert_eq!(parse_branch_mode("current").unwrap(), BranchMode::Current);
}

#[test]
fn parse_branch_mode_rejects_unknown() {
    let err = parse_branch_mode("sideways").unwrap_err();
    assert!(
        err.to_string().contains("must be 'new' or 'current'"),
        "got: {err}"
    );
}

// --- config handler round-trip ---

#[test]
fn handler_round_trip() {
    let (ws, dir) = tmp_ws("handler-round-trip");

    // set stores the value.
    set(&ws, "opencode.model", "kimi-for-coding/k2p7").unwrap();
    let s = Settings::load(&ws).unwrap();
    let o: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION).unwrap();
    assert_eq!(o.model.as_deref(), Some("kimi-for-coding/k2p7"));

    // unset clears it.
    unset(&ws, "opencode.model").unwrap();
    let s = Settings::load(&ws).unwrap();
    let o: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION).unwrap();
    assert_eq!(o.model, None);

    // Unknown key errors.
    let err = set(&ws, "bad.key", "x").unwrap_err();
    assert!(err.to_string().contains("unknown config key"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn new_keys_handler_round_trip() {
    let (ws, dir) = tmp_ws("new-keys-handler");

    set(&ws, "base_branch", "origin/dev").unwrap();
    set(&ws, "claude.max_minutes_per_issue", "45").unwrap();
    set(&ws, "branch_mode", "current").unwrap();

    let s = Settings::load(&ws).unwrap();
    let c: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION).unwrap();
    assert_eq!(s.base_branch.as_deref(), Some("origin/dev"));
    assert_eq!(c.max_minutes_per_issue, Some(45));
    assert_eq!(s.branch_mode.as_deref(), Some("current"));

    unset(&ws, "base_branch").unwrap();
    unset(&ws, "claude.max_minutes_per_issue").unwrap();
    unset(&ws, "branch_mode").unwrap();
    let s = Settings::load(&ws).unwrap();
    let c: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION).unwrap();
    assert_eq!(s.base_branch, None);
    assert_eq!(c.max_minutes_per_issue, None);
    assert_eq!(s.branch_mode, None);

    // Invalid branch_mode and non-integer budget are rejected at set time.
    let err = set(&ws, "branch_mode", "sideways").unwrap_err();
    assert!(
        err.to_string().contains("must be 'new' or 'current'"),
        "got: {err}"
    );
    let err = set(&ws, "claude.max_minutes_per_issue", "abc").unwrap_err();
    assert!(
        err.to_string().contains("must be a non-negative integer"),
        "got: {err}"
    );
    // `0` is a valid value — it disables the per-issue cap.
    set(&ws, "claude.max_minutes_per_issue", "0").unwrap();
    let c: ClaudeSettings = Settings::load(&ws)
        .unwrap()
        .agent_settings(ClaudeSettings::SECTION)
        .unwrap();
    assert_eq!(c.max_minutes_per_issue, Some(0));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn queue_assignee_config_round_trip() {
    let (ws, dir) = tmp_ws("queue-assignee-config");

    set(&ws, "queue.assignee", "@me").unwrap();
    let s = Settings::load(&ws).unwrap();
    assert_eq!(s.queue.assignee.as_deref(), Some("@me"));

    unset(&ws, "queue.assignee").unwrap();
    let s = Settings::load(&ws).unwrap();
    assert_eq!(s.queue.assignee, None);

    fs::remove_dir_all(&dir).ok();
}

/// The comment-trust opt-out (F8) is a bool like `verify.require_verify_gate`:
/// only `true`/`false` are accepted, unset reads back as `None` (the filter on).
#[test]
fn queue_trust_all_comments_config_round_trip() {
    let (ws, dir) = tmp_ws("queue-trust-config");

    set(&ws, "queue.trust_all_comments", "true").unwrap();
    let s = Settings::load(&ws).unwrap();
    assert_eq!(s.queue.trust_all_comments, Some(true));

    let err = set(&ws, "queue.trust_all_comments", "yes").unwrap_err();
    assert!(err.to_string().contains("'true' or 'false'"), "{err}");

    unset(&ws, "queue.trust_all_comments").unwrap();
    let s = Settings::load(&ws).unwrap();
    assert_eq!(s.queue.trust_all_comments, None);

    fs::remove_dir_all(&dir).ok();
}

/// A `.ralphy/settings.json` written by a pre-#79 binary (typed vendor
/// fields in core) must still parse, resolve with the ADR-0010 precedence
/// (flag > settings > default), and survive a typed save without losing
/// the vendor sections or unknown peer keys.
#[test]
fn previous_version_settings_file_still_parses_with_precedence() {
    let (ws, dir) = tmp_ws("back-compat");

    // Byte-for-byte shape a pre-#79 `save` produced (typed fields first,
    // including sections a config touch left present).
    let raw = r#"{
  "opencode": { "model": "kimi-for-coding/k2p7" },
  "base_branch": "origin/dev",
  "branch_mode": "current",
  "claude": { "plan_model": "opus", "max_minutes_per_issue": 45 },
  "verify": { "command": "cargo test" },
  "future_key": 123
}"#;
    fs::create_dir_all(ws.ralphy_dir()).unwrap();
    fs::write(ws.settings_path(), raw).unwrap();

    let s = Settings::load(&ws).unwrap();
    let c: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION).unwrap();
    let o: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION).unwrap();
    assert_eq!(o.model.as_deref(), Some("kimi-for-coding/k2p7"));
    assert_eq!(c.plan_model.as_deref(), Some("opus"));
    assert_eq!(c.max_minutes_per_issue, Some(45));
    assert_eq!(s.base_branch.as_deref(), Some("origin/dev"));
    assert_eq!(s.branch_mode.as_deref(), Some("current"));
    assert_eq!(s.verify.command.as_deref(), Some("cargo test"));

    // ADR-0010 precedence: flag > settings > default.
    assert_eq!(
        resolve_str(Some("flag".into()), c.plan_model.clone(), "default"),
        "flag"
    );
    assert_eq!(resolve_str(None, c.plan_model.clone(), "default"), "opus");
    assert_eq!(resolve_u64(None, c.max_minutes_per_issue, 90), 45);
    // A field the file never set falls through to the hardcoded default.
    assert_eq!(
        resolve_str(None, c.default_exec_model.clone(), "sonnet"),
        "sonnet"
    );

    // A typed save keeps the vendor sections and the unknown peer key.
    s.save(&ws).unwrap();
    let back = fs::read_to_string(ws.settings_path()).unwrap();
    for needle in [
        "opencode",
        "kimi-for-coding/k2p7",
        "claude",
        "plan_model",
        "future_key",
    ] {
        assert!(
            back.contains(needle),
            "missing '{needle}' after save:\n{back}"
        );
    }

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn get_json_round_trips() {
    let (ws, dir) = tmp_ws("get-json");
    set(&ws, "branch_mode", "new").unwrap();

    let v = config_json(&ws).unwrap();
    assert_eq!(v["branch_mode"], "new", "the set value round-trips: {v}");
    // Every supported key is present; an unset one is JSON null, not missing.
    assert!(
        v.get("opencode.model").map(|x| x.is_null()) == Some(true),
        "opencode.model present as null: {v}"
    );
    // The object parses as a plain map (the daemon Query verb re-parses it).
    assert!(v.is_object());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_json_masks_events_token() {
    // `events.token` must be MASKED in the JSON the daemon Query verb serves —
    // never the full secret. Serialize with the events-store tests (global env).
    let _g = crate::events::config::ENV_LOCK.lock().unwrap();
    let (ws, dir) = tmp_ws("json-mask");
    let store_dir = dir.join("store");
    std::env::set_var("RALPHY_EVENTS_DIR", &store_dir);

    set(&ws, "events.token", "supersecrettoken").unwrap();
    let v = config_json(&ws).unwrap();
    let tok = v["events.token"].as_str().expect("masked token string");
    assert_ne!(tok, "supersecrettoken", "the full token must not leak");
    assert_eq!(
        tok,
        crate::telegram::config::masked_token("supersecrettoken"),
        "the JSON carries the masked form"
    );

    std::env::remove_var("RALPHY_EVENTS_DIR");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn config_set_refuses_under_held_lock() {
    // `config set`/`config unset` are Mutate verbs (ADR-0036 §2/§6): `run()`
    // guards them with the shared run-lock guard before touching settings.
    // A held-alive lock refuses with the verb in the message.
    let (ws, dir) = tmp_ws("config-lock");
    fs::create_dir_all(ws.ralphy_dir()).unwrap();
    let stored = crate::runlock::LockInfo {
        pid: 4_000_000,
        started_at: "2026-07-13T10:00:00-03:00".into(),
    };
    fs::write(ws.run_lock_path(), serde_json::to_string(&stored).unwrap()).unwrap();

    for verb in ["config set", "config unset"] {
        let err = crate::runlock::guard_run_lock(&ws, verb, |pid| pid == 4_000_000)
            .unwrap_err()
            .to_string();
        assert!(err.contains(&format!("refusing to {verb}")), "got: {err}");
    }

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn help_notes_claude_only() {
    assert!(supported_keys_help().contains("Claude-only today"));
}

#[test]
fn verify_command_round_trip() {
    let (ws, dir) = tmp_ws("verify-command");

    set(&ws, "verify.command", "cargo test").unwrap();
    let s = Settings::load(&ws).unwrap();
    assert_eq!(s.verify.command.as_deref(), Some("cargo test"));

    unset(&ws, "verify.command").unwrap();
    let s = Settings::load(&ws).unwrap();
    assert_eq!(s.verify.command, None);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn help_lists_verify_command() {
    assert!(supported_keys_help().contains("verify.command"));
}

#[test]
fn help_lists_events_keys() {
    assert!(supported_keys_help().contains("events.url"));
    assert!(supported_keys_help().contains("events.token"));
}

/// Every registry key is covered by validation, help, and all three
/// `set`/`unset`/`get` handlers. A key added to `SUPPORTED_KEYS` without its
/// typed handler arm hits `unreachable!()` on `set`/`unset` → panics here.
#[test]
fn every_registry_key_is_handled_by_all_subcommands() {
    // `events.*` keys write the process-global store — serialize with the
    // events-store tests and point it at a temp dir.
    let _g = crate::events::config::ENV_LOCK.lock().unwrap();
    let (ws, dir) = tmp_ws("registry-coverage");
    let store_dir = dir.join("store");
    std::env::set_var("RALPHY_EVENTS_DIR", &store_dir);

    // A type-valid sample value per key (defaults to "x").
    let sample = |key: &str| -> &str {
        match key {
            "branch_mode" => "current",
            "verify.require_verify_gate" | "queue.trust_all_comments" => "true",
            "remote_control" => "true",
            "claude.max_minutes_per_issue" => "45",
            "claude.console_name" => "true",
            // Validated against effort vocabularies, so `x` is refused.
            "claude.plan_effort"
            | "claude.exec_effort"
            | "copilot.plan_effort"
            | "copilot.exec_effort" => "high",
            "copilot.allow_builtin_mcp_servers_i_understand_the_risk" => "true",
            "cursor.allow_codebase_indexing_i_understand_the_risk" => "true",
            // Validated against the vendor's id set, so `x` is refused.
            "gemini.plan_model" | "gemini.exec_model" => "gemini-3.5-flash",
            _ => "x",
        }
    };

    for k in SUPPORTED_KEYS {
        assert!(
            require_known_key(k).is_ok(),
            "{k} not accepted by validation"
        );
        assert!(
            supported_keys_help().contains(k),
            "{k} missing from help listing"
        );
        set(&ws, k, sample(k)).unwrap_or_else(|e| panic!("set {k}: {e}"));
    }
    get(&ws, false).unwrap();
    get(&ws, true).unwrap();
    for k in SUPPORTED_KEYS {
        unset(&ws, k).unwrap_or_else(|e| panic!("unset {k}: {e}"));
    }

    std::env::remove_var("RALPHY_EVENTS_DIR");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn events_keys_write_global_store_not_settings_json() {
    // Serialize with the events-store tests: `RALPHY_EVENTS_DIR` is global.
    let _g = crate::events::config::ENV_LOCK.lock().unwrap();
    let (ws, dir) = tmp_ws("events-routing");
    let store_dir = dir.join("store");
    std::env::set_var("RALPHY_EVENTS_DIR", &store_dir);

    // `set events.url` writes the global store and never `.ralphy/settings.json`.
    set(&ws, "events.url", "http://x/hook").unwrap();
    set(&ws, "events.token", "sekret").unwrap();
    assert!(
        !ws.settings_path().exists(),
        "events keys must not create settings.json"
    );
    assert!(
        store_dir.join("events.toml").exists(),
        "events.toml must be written to the global store"
    );

    // The value round-trips for this repo's slug.
    let slug = git::project_slug(ws.repo_root());
    let store = crate::events::config::EventsStore::load().unwrap();
    let entry = store.entry(&slug).expect("entry present");
    assert_eq!(entry.url.as_deref(), Some("http://x/hook"));
    assert_eq!(entry.token.as_deref(), Some("sekret"));

    // `unset` clears the field in the store; settings.json still never appears.
    unset(&ws, "events.token").unwrap();
    let store = crate::events::config::EventsStore::load().unwrap();
    assert!(store.entry(&slug).unwrap().token.is_none());
    assert!(!ws.settings_path().exists());

    std::env::remove_var("RALPHY_EVENTS_DIR");
    fs::remove_dir_all(&dir).ok();
}
