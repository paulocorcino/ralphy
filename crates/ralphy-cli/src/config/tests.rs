use super::test_support::{remove_scratch, tmp_ws};
use super::*;
use ralphy_agent_opencode::OpenCodeSettings;
use std::fs;

mod agents;

/// `config set` stores a key and `config unset` clears it, per key; a boolean
/// key refuses anything but `true`/`false`, and an unknown key is refused.
#[test]
fn config_keys_round_trip_through_set_and_unset() {
    type Read = fn(&Settings) -> String;
    // (key, value, read the stored value as Debug text, the Debug text once
    // set, a refused value and the words of its refusal)
    type Row<'a> = (&'a str, &'a str, Read, &'a str, Option<(&'a str, &'a str)>);
    let rows: [Row<'_>; 7] = [
        (
            "remote_control",
            "true",
            |s| format!("{:?}", s.remote_control),
            "Some(true)",
            Some(("maybe", "must be 'true' or 'false'")),
        ),
        (
            "opencode.model",
            "kimi-for-coding/k2p7",
            |s| {
                let o: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION).unwrap();
                format!("{:?}", o.model)
            },
            "Some(\"kimi-for-coding/k2p7\")",
            None,
        ),
        (
            "queue.assignee",
            "@me",
            |s| format!("{:?}", s.queue.assignee),
            "Some(\"@me\")",
            None,
        ),
        (
            "queue.trust_all_comments",
            "true",
            |s| format!("{:?}", s.queue.trust_all_comments),
            "Some(true)",
            Some(("yes", "'true' or 'false'")),
        ),
        (
            "verify.command",
            "cargo test",
            |s| format!("{:?}", s.verify.command),
            "Some(\"cargo test\")",
            None,
        ),
        (
            "runs.keep",
            "7",
            |s| format!("{:?}", s.runs.keep),
            "Some(7)",
            None,
        ),
        (
            "files.upload_max_mb",
            "100",
            |s| format!("{:?}", s.files.upload_max_mb),
            "Some(100)",
            Some(("0", "at least 1")),
        ),
    ];
    for (key, value, read, stored, refused) in rows {
        let (ws, dir) = tmp_ws(&key.replace('.', "-"));
        set(&ws, key, value).unwrap();
        assert_eq!(read(&Settings::load(&ws).unwrap()), stored, "{key}: set");
        if let Some((bad, words)) = refused {
            let err = set(&ws, key, bad).unwrap_err();
            assert!(err.to_string().contains(words), "{key}: {err}");
        }
        unset(&ws, key).unwrap();
        assert_eq!(read(&Settings::load(&ws).unwrap()), "None", "{key}: unset");
        remove_scratch(&dir);
    }

    let (ws, dir) = tmp_ws("unknown-key");
    let err = set(&ws, "bad.key", "x").unwrap_err();
    assert!(err.to_string().contains("unknown config key"), "{err}");
    remove_scratch(&dir);
}

#[test]
fn runs_keep_refuses_a_value_that_is_not_a_non_negative_integer() {
    let (ws, dir) = tmp_ws("runs-keep-refused");
    for bad in ["-1", "abc"] {
        let err = set(&ws, "runs.keep", bad).unwrap_err();
        assert!(
            err.to_string()
                .contains("runs.keep must be a non-negative integer"),
            "{bad}: {err}"
        );
    }
    assert_eq!(Settings::load(&ws).unwrap().runs.keep, None);
    remove_scratch(&dir);
}

// --- config handler round-trip ---
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

    remove_scratch(&dir);
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

    remove_scratch(&dir);
}

#[test]
fn get_json_round_trips() {
    let (ws, dir) = tmp_ws("get-json");
    set(&ws, "branch_mode", "new").unwrap();

    let v = config_json(&ws).unwrap();
    assert_eq!(v["branch_mode"], "new", "the set value round-trips: {v}");
    // Every supported key is present; an unset one is JSON null, not missing.
    assert!(
        v.get("opencode.model")
            .is_some_and(serde_json::Value::is_null),
        "opencode.model present as null: {v}"
    );
    // The object parses as a plain map (the daemon Query verb re-parses it).
    assert!(v.is_object());

    remove_scratch(&dir);
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
    remove_scratch(&dir);
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
            "verify.require_verify_gate"
            | "queue.trust_all_comments"
            | "remote_control"
            | "claude.console_name"
            | "claude.console_classic_screen"
            | "copilot.allow_builtin_mcp_servers_i_understand_the_risk"
            | "cursor.allow_codebase_indexing_i_understand_the_risk"
            | "claude.run_without_guard_proof_i_understand_the_risk"
            | "antigravity.run_without_guard_proof_i_understand_the_risk" => "true",
            "claude.max_minutes_per_issue" => "45",
            "runs.keep" => "50",
            "files.upload_max_mb" => "25",
            // Validated against effort vocabularies, so `x` is refused.
            "claude.plan_effort"
            | "claude.exec_effort"
            | "copilot.plan_effort"
            | "copilot.exec_effort"
            | "antigravity.plan_effort"
            | "antigravity.exec_effort" => "high",
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
    remove_scratch(&dir);
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
    remove_scratch(&dir);
}

/// The cleartext-sink warning (audit F14) fires for exactly one shape: an
/// `http://` URL whose host is not loopback. `https://` and a loopback
/// `http://` say nothing; a non-URL is left to the sink's own validation.
#[test]
fn cleartext_remote_host_names_only_a_plain_http_remote() {
    assert_eq!(
        cleartext_remote_host("http://sink.example/hook"),
        Some("sink.example")
    );
    assert_eq!(
        cleartext_remote_host("http://10.0.0.7:8080/hook"),
        Some("10.0.0.7")
    );
    assert_eq!(
        cleartext_remote_host("http://user:pw@sink.example/x"),
        Some("sink.example")
    );
    assert_eq!(
        cleartext_remote_host("http://[2001:db8::1]:9/x"),
        Some("2001:db8::1")
    );
    assert_eq!(cleartext_remote_host("https://sink.example/hook"), None);
    assert_eq!(cleartext_remote_host("http://localhost:7257/hook"), None);
    assert_eq!(cleartext_remote_host("http://127.0.0.1/hook"), None);
    assert_eq!(cleartext_remote_host("http://[::1]:9/hook"), None);
    assert_eq!(cleartext_remote_host("not a url"), None);
}
