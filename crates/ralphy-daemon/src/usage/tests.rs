use super::*;

/// Serializes the env-mutating path resolvers against each other (mirrors
/// `identity.rs`'s lock); the tests share one process env.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn write_ledger(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

fn descriptor(id: &str, environment: &str) -> crate::peer::PeerDescriptor {
    crate::peer::PeerDescriptor {
        daemon_id: id.to_string(),
        name: "peer".to_string(),
        avatar: "🐙".to_string(),
        address: "127.0.0.1".to_string(),
        port: 1,
        environment: environment.to_string(),
        token: "token".to_string(),
        protocol_version: crate::peer::PEER_PROTOCOL_VERSION,
        nudge: None,
    }
}

#[test]
fn fleet_fold_stamps_sources_and_names_failed_or_malformed_peers() {
    let local = UsageContribution {
        daemon_id: Some("local".to_string()),
        records: vec![serde_json::json!({
            "session_id": "local-session",
            "daemon_id": "run-emitted"
        })],
        interactive: vec![],
    };
    let peer = UsageContribution {
        daemon_id: Some("untrusted-response-id".to_string()),
        records: vec![serde_json::json!({ "session_id": "peer-session" })],
        interactive: vec![serde_json::json!({
            "session_id": "peer-existing",
            "daemon_id": "interactive-emitted"
        })],
    };
    let malformed = serde_json::from_slice::<UsageContribution>(br#"{"records":"not-an-array"}"#)
        .map_err(|error| format!("invalid peer usage data: {error}"));
    let fleet = fold_fleet_usage(
        local,
        [
            (descriptor("peer", "WSL: Ubuntu-22.04"), Ok(peer)),
            (descriptor("malformed", "Linux"), malformed),
            (
                descriptor("failed", "Windows"),
                Err("connection refused".to_string()),
            ),
        ],
        [],
    );

    assert_eq!(fleet.records[0]["daemon_id"], "run-emitted");
    assert_eq!(fleet.records[1]["daemon_id"], "peer");
    assert_eq!(fleet.interactive[0]["daemon_id"], "interactive-emitted");
    assert_eq!(fleet.missing.len(), 2);
    assert_eq!(fleet.missing[0].daemon_id, "malformed");
    assert!(fleet.missing[0].why.contains("invalid peer usage data"));
    assert_eq!(fleet.missing[1].daemon_id, "failed");
    assert_eq!(fleet.missing[1].why, "connection refused");
}

#[test]
fn fleet_fold_names_incompatible_announced_peers() {
    let rejected = crate::peer::PeerReject::IncompatibleVersion {
        file: "peer.toml".to_string(),
        daemon_id: "old-peer".to_string(),
        environment: "WSL: Debian".to_string(),
        theirs: crate::peer::PEER_PROTOCOL_VERSION - 1,
    };
    let fleet = fold_fleet_usage(
        UsageContribution {
            daemon_id: Some("local".to_string()),
            records: vec![],
            interactive: vec![],
        },
        [],
        [rejected],
    );

    assert_eq!(fleet.missing.len(), 1);
    assert_eq!(fleet.missing[0].daemon_id, "old-peer");
    assert_eq!(fleet.missing[0].environment, "WSL: Debian");
    assert!(fleet.missing[0].why.contains("speaks peer protocol"));
}

/// The Ledger grid is scoped to the OPEN project, so a second project's rows
/// must leave through BOTH inputs — while `missing` survives, because a peer
/// that never answered is fleet health rather than project data.
#[test]
fn scoping_to_a_project_keeps_only_its_rows_and_never_touches_missing() {
    let mut fleet = FleetUsage {
        daemon_id: Some("local".to_string()),
        records: vec![
            serde_json::json!({ "project": "owner/repo", "session_id": "a" }),
            serde_json::json!({ "project": "other/repo", "session_id": "b" }),
            // No `project` field at all: not this project's, so it goes.
            serde_json::json!({ "session_id": "c" }),
        ],
        interactive: vec![
            serde_json::json!({ "project": "other/repo", "session_id": "i1" }),
            serde_json::json!({ "project": "owner/repo", "session_id": "i2" }),
        ],
        missing: vec![MissingUsageContribution {
            daemon_id: "peer".to_string(),
            environment: "WSL: Ubuntu-22.04".to_string(),
            why: "connecting".to_string(),
        }],
    };

    scope_to_project(&mut fleet, "owner/repo");

    assert_eq!(fleet.records.len(), 1);
    assert_eq!(fleet.records[0]["session_id"], "a");
    assert_eq!(fleet.interactive.len(), 1);
    assert_eq!(fleet.interactive[0]["session_id"], "i2");
    assert_eq!(
        fleet.missing.len(),
        1,
        "a peer that never answered is fleet health, not project data"
    );
}

/// `$RALPHY_COPILOT_DB` wins over `$COPILOT_HOME`, which wins over the home
/// default. Env is process-global, so the three legs run in one test.
#[test]
fn copilot_db_path_prefers_the_env_override() {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let restore = (
        std::env::var_os("RALPHY_COPILOT_DB"),
        std::env::var_os("COPILOT_HOME"),
    );

    std::env::set_var("RALPHY_COPILOT_DB", "C:/tmp/override.db");
    std::env::set_var("COPILOT_HOME", "C:/tmp/copilot-home");
    assert_eq!(
        copilot_db_path().unwrap(),
        PathBuf::from("C:/tmp/override.db")
    );

    std::env::remove_var("RALPHY_COPILOT_DB");
    assert_eq!(
        copilot_db_path().unwrap(),
        PathBuf::from("C:/tmp/copilot-home").join("session-store.db")
    );

    std::env::remove_var("COPILOT_HOME");
    let home = copilot_db_path().unwrap();
    assert!(
        home.ends_with(PathBuf::from(".copilot").join("session-store.db")),
        "home default, got {home:?}"
    );

    match restore.0 {
        Some(v) => std::env::set_var("RALPHY_COPILOT_DB", v),
        None => std::env::remove_var("RALPHY_COPILOT_DB"),
    }
    match restore.1 {
        Some(v) => std::env::set_var("COPILOT_HOME", v),
        None => std::env::remove_var("COPILOT_HOME"),
    }
    drop(guard);
}

/// D17 points `$CURSOR_CONFIG_DIR` at Ralphy's per-run SCRATCH directory. If
/// this resolver honoured it, the daemon would report on Ralphy's own throwaway
/// state instead of the operator's sessions — so the scratch var must not divert
/// it, while the test-only `$RALPHY_CURSOR_DIR` still wins.
#[test]
fn cursor_dir_path_ignores_the_scratch_config_dir() {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let restore = (
        std::env::var_os("CURSOR_CONFIG_DIR"),
        std::env::var_os("RALPHY_CURSOR_DIR"),
        std::env::var_os("XDG_CONFIG_HOME"),
    );

    std::env::set_var("CURSOR_CONFIG_DIR", "C:/tmp/ralphy-scratch");
    std::env::remove_var("RALPHY_CURSOR_DIR");
    std::env::remove_var("XDG_CONFIG_HOME");
    let got = cursor_dir_path().unwrap();
    assert!(
        got.ends_with(".cursor"),
        "the scratch config dir must not divert the resolver, got {got:?}"
    );
    assert!(
        !got.starts_with("C:/tmp/ralphy-scratch"),
        "resolved Ralphy's own scratch state, got {got:?}"
    );

    std::env::set_var("RALPHY_CURSOR_DIR", "C:/tmp/override");
    assert_eq!(
        cursor_dir_path().unwrap(),
        PathBuf::from("C:/tmp/override"),
        "the test override must still win"
    );

    match restore.0 {
        Some(v) => std::env::set_var("CURSOR_CONFIG_DIR", v),
        None => std::env::remove_var("CURSOR_CONFIG_DIR"),
    }
    match restore.1 {
        Some(v) => std::env::set_var("RALPHY_CURSOR_DIR", v),
        None => std::env::remove_var("RALPHY_CURSOR_DIR"),
    }
    match restore.2 {
        Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
        None => std::env::remove_var("XDG_CONFIG_HOME"),
    }
    drop(guard);
}

/// ADR-0043 D4 points `$GEMINI_CLI_HOME` at Ralphy's OWN owned root. If this
/// resolver honoured it, the daemon would report Ralphy's per-repo state instead
/// of the operator's sessions — so it must not divert the resolver, while the
/// test-only `$RALPHY_GEMINI_DIR` still wins.
#[test]
fn gemini_dir_path_ignores_ralphys_own_cli_home() {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let restore = (
        std::env::var_os("GEMINI_CLI_HOME"),
        std::env::var_os("RALPHY_GEMINI_DIR"),
    );

    std::env::set_var("GEMINI_CLI_HOME", "C:/tmp/ralphy-owned-root");
    std::env::remove_var("RALPHY_GEMINI_DIR");
    let got = gemini_dir_path().unwrap();
    assert!(
        got.ends_with(".gemini"),
        "Ralphy's own CLI home must not divert the resolver, got {got:?}"
    );
    assert!(
        !got.starts_with("C:/tmp/ralphy-owned-root"),
        "resolved Ralphy's own owned root, got {got:?}"
    );

    std::env::set_var("RALPHY_GEMINI_DIR", "C:/tmp/override");
    assert_eq!(
        gemini_dir_path().unwrap(),
        PathBuf::from("C:/tmp/override"),
        "the test override must still win"
    );

    match restore.0 {
        Some(v) => std::env::set_var("GEMINI_CLI_HOME", v),
        None => std::env::remove_var("GEMINI_CLI_HOME"),
    }
    match restore.1 {
        Some(v) => std::env::set_var("RALPHY_GEMINI_DIR", v),
        None => std::env::remove_var("RALPHY_GEMINI_DIR"),
    }
    drop(guard);
}

/// ADR-0040 Tier 4 anti-drift: a vendor that reaches the daemon's launch enum
/// must have a store-path RESOLVER here AND have its scan actually chained into
/// [`interactive_records`]. Source-text pin over this very file, so it reds the
/// moment a seventh `Agent::ALL` variant lands with a resolver nobody calls —
/// the state Cursor was left in by #248 and that #250 closed.
#[test]
fn every_launchable_vendor_has_a_store_path_resolver() {
    let src = include_str!("../usage.rs");
    for agent in crate::session::Agent::ALL {
        let token = crate::dispatch::agent_flag(agent);
        let found = src.lines().any(|l| {
            l.trim_start()
                .strip_prefix("pub fn ")
                .is_some_and(|rest| rest.starts_with(token) && rest.contains("_path("))
        });
        assert!(
            found,
            "no `pub fn {token}…_path(` resolver in usage.rs — {agent:?} reached \
                 the daemon's launch enum without one, so nothing can even locate \
                 its interactive store"
        );
        assert!(
            src.contains(&format!("scan_{token}(&")),
            "no `scan_{token}(&` call in usage.rs — {agent:?} has a store-path \
                 resolver but its scan is never chained into `interactive_records`, \
                 so /api/usage reports none of its interactive sessions"
        );
    }
}

#[test]
fn run_records_returns_all_lines_when_since_is_none() {
    let dir = tempfile::tempdir().unwrap();
    write_ledger(
        dir.path(),
        "owner-repo.jsonl",
        "{\"session_id\":\"sess-a\",\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
             {\"session_id\":\"sess-b\",\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
    );
    let records = run_records(dir.path(), None);
    assert_eq!(records.len(), 2);
}

#[test]
fn run_records_since_filters_to_matching_or_later_ts() {
    let dir = tempfile::tempdir().unwrap();
    write_ledger(
        dir.path(),
        "owner-repo.jsonl",
        "{\"session_id\":\"sess-a\",\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
             {\"session_id\":\"sess-b\",\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
    );
    let records = run_records(dir.path(), Some("2026-06-15T12:05:00+00:00"));
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].get("session_id").and_then(|v| v.as_str()),
        Some("sess-b")
    );
}

#[test]
fn run_records_skips_a_malformed_middle_line() {
    let dir = tempfile::tempdir().unwrap();
    write_ledger(
        dir.path(),
        "owner-repo.jsonl",
        "{\"session_id\":\"sess-a\",\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
             { this is not valid json\n\
             {\"session_id\":\"sess-b\",\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
    );
    let records = run_records(dir.path(), None);
    assert_eq!(records.len(), 2, "malformed middle line is skipped");
}

#[test]
fn local_contribution_projects_recovered_models_without_mutating_files() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = dir.path().join("owner-repo.jsonl");
    let ledger_bytes = concat!(
        "{\"model\":\"unknown\",\"session_id\":\"sess-a\",\"ts\":\"1\"}\n",
        "{\"model\":\"known\",\"session_id\":\"sess-a\",\"ts\":\"2\"}\n",
    )
    .as_bytes()
    .to_vec();
    let map = br#"{"sess-a":"claude-opus-4-8"}"#.to_vec();
    std::fs::write(&ledger, &ledger_bytes).unwrap();
    std::fs::write(dir.path().join("session-models.json"), &map).unwrap();

    let contribution = local_contribution(
        dir.path(),
        &StorePaths::default(),
        &RegistryStore::default(),
        None,
        None,
    );

    assert_eq!(contribution.records[0]["model"], "claude-opus-4-8");
    assert_eq!(contribution.records[1]["model"], "known");
    assert_eq!(std::fs::read(&ledger).unwrap(), ledger_bytes);
    assert_eq!(
        std::fs::read(dir.path().join("session-models.json")).unwrap(),
        map
    );
}

#[test]
fn malformed_or_missing_model_map_degrades_to_raw_record() {
    for map in [None, Some(b"not json".as_slice())] {
        let dir = tempfile::tempdir().unwrap();
        write_ledger(
            dir.path(),
            "owner-repo.jsonl",
            "{\"model\":\"unknown\",\"session_id\":\"sess-a\",\"ts\":\"1\"}\n",
        );
        if let Some(bytes) = map {
            std::fs::write(dir.path().join("session-models.json"), bytes).unwrap();
        }
        let contribution = local_contribution(
            dir.path(),
            &StorePaths::default(),
            &RegistryStore::default(),
            None,
            None,
        );
        assert_eq!(contribution.records[0]["model"], "unknown");
    }
}

/// #262's whole deliverable is the LABEL, and it lives in JS/HTML that no
/// Rust gate compiles: deleting the mark or the caveat leaves the suite green
/// while the operator reads a floor as a total (ADR-0043 D10). Pins both
/// renderers into the served assets, like `dispatch.rs`'s workbench-trio pin
/// does for the agent list. #360 moved the surface from the Usage modal to
/// the Spend tab's Ledger grid; the GUARANTEE is the same, so this test
/// followed it rather than being deleted with its old host.
#[test]
fn the_workbench_labels_a_lower_bound_record() {
    let js = include_str!("../../assets/ui/wb-spend.js");
    let start = js
        .find("function boundMark(")
        .expect("wb-spend.js: boundMark moved");
    let body = &js[start..start + 400];
    // The quotes are part of the needle: a comment mentioning the glyph must
    // not be able to satisfy a pin on the code that emits it.
    assert!(
        body.contains("\"\u{2265} \" + value"),
        "boundMark must prefix a lower-bound count with `\u{2265} `: {body}"
    );
    assert!(
        js.contains("\" (lower bound)\""),
        "wb-spend.js must still say `(lower bound)` in words beside the row"
    );
    assert!(
        js.contains("lowerBound: !!rec.lower_bound")
            && js.contains("counts(rec.tokens, !!rec.lower_bound)"),
        "a ledger row must read `lower_bound` off the record and carry it \
             into its counts — the caveat rides on the NUMBER"
    );

    let html = include_str!("../../assets/ui/index.html");
    assert!(
        html.contains("x-show=\"ledgerView().anyLowerBound\""),
        "index.html must show the caveat note only when a row is a floor"
    );
    assert!(
        html.contains("&#8805; means the real cost is at least this amount"),
        "index.html must explain what the \u{2265} means"
    );
}

/// The Usage modal is REPLACED by the Spend tab's Ledger pane (PRD #355:
/// "exactly one place the cost lives"), not left to coexist. Dead markup and
/// dead handlers are how two surfaces quietly come back — and a stale
/// `openUsage()` in the account dropdown would open nothing at all.
#[test]
fn the_usage_modal_is_gone_from_the_served_assets() {
    // The stylesheet is twelve partials under `assets/ui/styles/`
    // (ADR-0057), so the sweep reads what the browser assembles rather than
    // one file — and it walks the DIRECTORY, so a thirteenth partial is
    // swept the day it is added.
    let stylesheet = crate::UI
        .get_dir("styles")
        .expect("the stylesheet partials must be embedded")
        .files()
        .filter(|f| f.path().extension().is_some_and(|e| e == "css"))
        .map(|f| f.contents_utf8().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    // A sweep over an empty string passes every "does not contain" it makes,
    // so state that the bytes are there before asserting about them.
    assert!(
        stylesheet.len() > 100_000,
        "the assembled stylesheet is {} bytes — the sweep below would be \
             passing over nothing",
        stylesheet.len()
    );
    let assets = [
        ("index.html", include_str!("../../assets/ui/index.html")),
        ("app.js", include_str!("../../assets/ui/app.js")),
        ("the stylesheet", stylesheet.as_str()),
    ];
    for needle in [
        "openUsage",
        "usageOpen",
        "closeUsage",
        "usageTokens",
        "usage-modal",
        "usage-table",
        "usage-row",
        "usage-body",
        "usage-section",
    ] {
        for (name, source) in assets {
            assert_eq!(
                source.matches(needle).count(),
                0,
                "{name} still carries the removed Usage modal's `{needle}`"
            );
        }
    }
}
