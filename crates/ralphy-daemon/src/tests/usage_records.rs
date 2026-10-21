//! The interactive records `/api/usage` carries for each vendor.

use super::*;

/// `/api/usage` now carries an `interactive` array from the Claude scan
/// alongside the ledger's `records`. A session the ledger already owns
/// (`run-sess`) is excluded from `interactive`; a genuinely-interactive one
/// (`int-sess`) appears. The scan runs against a temp store, so no operator
/// state is read.
#[tokio::test]
async fn api_usage_carries_run_and_interactive_records() {
    let usage_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        usage_dir.path().join("owner-repo.jsonl"),
        "{\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"plan\",\"session_id\":\"run-sess\",\"ts\":\"2026-06-15T12:00:00+00:00\"}\n",
    )
    .unwrap();

    let claude_dir = tempfile::tempdir().unwrap();
    let ws = claude_dir.path().join("ws-key");
    std::fs::create_dir_all(&ws).unwrap();
    let line = |id: &str| {
        format!(
            "{{\"requestId\":\"r1\",\"timestamp\":\"2026-07-10T10:00:00Z\",\"message\":{{\"id\":\"{id}\",\"model\":\"claude-opus-4-8\",\"usage\":{{\"input_tokens\":10,\"output_tokens\":1,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":0}}}}}}"
        )
    };
    std::fs::write(ws.join("run-sess.jsonl"), line("m1")).unwrap();
    std::fs::write(ws.join("int-sess.jsonl"), line("m2")).unwrap();

    let resp = router(
        None,
        PathBuf::from("does-not-exist"),
        usage_dir.path().to_path_buf(),
        StorePaths {
            claude_projects_dir: claude_dir.path().to_path_buf(),
            ..Default::default()
        },
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/usage")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let raw = resp.into_body().collect().await.unwrap().to_bytes();
    let body_string = String::from_utf8_lossy(&raw);
    let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();

    let interactive = body["interactive"].as_array().expect("interactive array");
    let has = |sid: &str| {
        interactive
            .iter()
            .any(|r| r.get("session_id").and_then(|v| v.as_str()) == Some(sid))
    };
    assert!(
        has("int-sess"),
        "interactive must carry int-sess; got: {body_string}"
    );
    assert!(
        !has("run-sess"),
        "the run-owned session must be excluded; got: {body_string}"
    );

    let records = body["records"].as_array().expect("records array");
    assert!(
        records
            .iter()
            .any(|r| r.get("session_id").and_then(|v| v.as_str()) == Some("run-sess")),
        "records must still carry the run line; got: {body_string}"
    );
    assert!(
        !body_string.contains("usd"),
        "no pricing in the payload; got: {body_string}"
    );

    // A vendor that writes every token to disk reports a total, not a floor:
    // a blanket `lower_bound: true` would mislabel the whole modal.
    let claude = interactive
        .iter()
        .find(|r| r.get("session_id").and_then(|v| v.as_str()) == Some("int-sess"))
        .unwrap();
    assert_eq!(claude["lower_bound"].as_bool(), Some(false), "{claude}");
}

/// `/api/usage` carries the interactive records of every seeded vendor store:
/// one Codex rollout, one OpenCode row, one Copilot row and one legacy Kimi
/// `wire.jsonl` flow through the scan and appear in the `interactive` array
/// with their agent and session id. Proves each store's router argument
/// (`codex_dir`, `opencode_db`, `copilot_db`, `kimi_dir`) is threaded end to
/// end, and that the payload carries no pricing.
#[tokio::test]
async fn api_usage_carries_each_vendor_stores_interactive_records() {
    use rusqlite::Connection;
    let tmp = tempfile::tempdir().unwrap();

    // Codex: a rollout under `sessions/YYYY/MM/DD`.
    let codex_dir = tmp.path().join("codex");
    let roll = codex_dir
        .join("sessions")
        .join("2026")
        .join("07")
        .join("10");
    std::fs::create_dir_all(&roll).unwrap();
    let meta_id = "019c5131-651b-78f2-b8e7-93995bff4dad";
    let body = format!(
        "{{\"timestamp\":\"2026-07-10T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{meta_id}\",\"cwd\":\"c:\\\\Dev\\\\x\"}}}}\n\
         {{\"timestamp\":\"2026-07-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{{\"model\":\"gpt-5.3-codex\"}}}}\n\
         {{\"timestamp\":\"2026-07-10T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":1000,\"cached_input_tokens\":800,\"output_tokens\":200}}}}}}}}\n"
    );
    std::fs::write(roll.join("rollout-int-abc.jsonl"), body).unwrap();

    // OpenCode: one assistant message in `opencode.db`.
    let opencode_db = tmp.path().join("opencode.db");
    {
        let conn = Connection::open(&opencode_db).unwrap();
        conn.execute(
            "CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)",
            [],
        )
        .unwrap();
        conn.execute("CREATE TABLE session (id TEXT, directory TEXT)", [])
            .unwrap();
        let data = r#"{"role":"assistant","modelID":"k2p6","tokens":{"input":2168,"output":100,"cache":{"write":0,"read":11264}}}"#;
        conn.execute(
            "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
            rusqlite::params!["msg_1", "ses_oc", data],
        )
        .unwrap();
    }

    // Copilot: one usage event in `session-store.db`.
    let copilot_db = tmp.path().join("session-store.db");
    {
        let conn = Connection::open(&copilot_db).unwrap();
        conn.execute(
            "CREATE TABLE assistant_usage_events (id INTEGER PRIMARY KEY AUTOINCREMENT, \
             session_id TEXT, model TEXT, input_tokens INTEGER, output_tokens INTEGER, \
             cache_read_tokens INTEGER, cache_write_tokens INTEGER, created_at TEXT)",
            [],
        )
        .unwrap();
        conn.execute("CREATE TABLE sessions (id TEXT PRIMARY KEY, cwd TEXT)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO assistant_usage_events (session_id, model, input_tokens, \
             output_tokens, cache_read_tokens, cache_write_tokens, created_at) \
             VALUES ('ses_cp', 'claude-sonnet-5', 22913, 350, 0, 22903, \
             '2026-07-20T11:54:33.066Z')",
            [],
        )
        .unwrap();
    }

    // Kimi: a legacy `wire.jsonl` with one non-zero `StatusUpdate`; the
    // session id is the parent directory's name.
    let kimi_dir = tmp.path().join("kimi");
    let sess = kimi_dir.join("sessions").join("GRP").join("SESS");
    std::fs::create_dir_all(&sess).unwrap();
    let line = "{\"timestamp\": 1770983410.0, \"message\": {\"type\": \"StatusUpdate\", \"payload\": {\"token_usage\": {\"input_other\": 100, \"output\": 10, \"input_cache_read\": 0, \"input_cache_creation\": 0}, \"message_id\": \"m1\"}}}";
    std::fs::write(sess.join("wire.jsonl"), line).unwrap();

    let resp = router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths {
            codex_dir,
            opencode_db,
            copilot_db,
            kimi_dir,
            ..Default::default()
        },
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/usage")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let raw = resp.into_body().collect().await.unwrap().to_bytes();
    let body_string = String::from_utf8_lossy(&raw);
    let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let interactive = body["interactive"].as_array().expect("interactive array");
    // (agent, session id)
    for (agent, session_id) in [
        ("codex", meta_id),
        ("opencode", "ses_oc"),
        ("copilot", "ses_cp"),
        ("kimi", "SESS"),
    ] {
        assert!(
            interactive.iter().any(|r| {
                r.get("agent").and_then(|v| v.as_str()) == Some(agent)
                    && r.get("session_id").and_then(|v| v.as_str()) == Some(session_id)
            }),
            "interactive must carry a {agent} record with session id {session_id}; got: {body_string}"
        );
    }
    assert!(
        !body_string.contains("usd"),
        "no pricing in the payload; got: {body_string}"
    );
}

/// `/api/usage` also carries Cursor interactive records, and their `tokens` is
/// JSON `null` — the key is PRESENT and is not `0` (ADR-0042 D11: Cursor keeps
/// no token count anywhere, so "unavailable" must not serialize as "spent
/// nothing"). Proves the `cursor_dir` router arg is threaded end-to-end.
#[tokio::test]
async fn api_usage_carries_cursor_interactive_records() {
    let cursor_dir = tempfile::tempdir().unwrap();
    let sid = "33333333-3333-3333-3333-333333333333";
    let sess = cursor_dir.path().join("chats").join("aaaa").join(sid);
    std::fs::create_dir_all(&sess).unwrap();
    std::fs::write(
        sess.join("meta.json"),
        r#"{"schemaVersion":1,"createdAtMs":1784593842510,"hasConversation":true,"updatedAtMs":1784593855173,"cwd":"C:\\Dev\\FinCal"}"#,
    )
    .unwrap();

    let resp = router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths {
            cursor_dir: cursor_dir.path().to_path_buf(),
            ..Default::default()
        },
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/usage")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let raw = resp.into_body().collect().await.unwrap().to_bytes();
    let body_string = String::from_utf8_lossy(&raw);
    let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let interactive = body["interactive"].as_array().expect("interactive array");
    let record = interactive
        .iter()
        .find(|r| {
            r.get("agent").and_then(|v| v.as_str()) == Some("cursor")
                && r.get("session_id").and_then(|v| v.as_str()) == Some(sid)
        })
        .unwrap_or_else(|| panic!("no cursor record for {sid}; got: {body_string}"));
    assert!(
        record.get("tokens").is_some(),
        "the tokens key must be PRESENT, not omitted; got: {record}"
    );
    assert!(
        record["tokens"].is_null(),
        "tokens must be null (unavailable), never 0; got: {record}"
    );
}

/// `/api/usage` also carries Gemini interactive records, with REAL counts —
/// unlike Cursor's `null`, the Gemini store keeps per-turn tokens (a lower
/// bound, ADR-0043 D10). Proves the `gemini_dir` router arg is threaded
/// end-to-end.
#[tokio::test]
async fn api_usage_carries_gemini_interactive_records() {
    let gemini_dir = tempfile::tempdir().unwrap();
    let chats = gemini_dir.path().join("tmp").join("fincal").join("chats");
    std::fs::create_dir_all(&chats).unwrap();
    std::fs::write(
        gemini_dir
            .path()
            .join("tmp")
            .join("fincal")
            .join(".project_root"),
        "c:\\dev\\fincal",
    )
    .unwrap();
    std::fs::write(
        chats.join("session-x.jsonl"),
        "{\"sessionId\":\"ralphy-probe-p1p2p3p4p6\",\"startTime\":\"2026-07-21T00:56:00Z\",\"lastUpdated\":\"2026-07-21T01:00:00Z\",\"kind\":\"main\"}\n\
         {\"id\":\"78d80d17\",\"type\":\"gemini\",\"content\":\"OK\",\"tokens\":{\"input\":20637,\"output\":30,\"cached\":0,\"thoughts\":257,\"tool\":0,\"total\":20924},\"model\":\"gemini-3.1-pro-preview-customtools\"}\n",
    )
    .unwrap();

    let resp = router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths {
            gemini_dir: gemini_dir.path().to_path_buf(),
            ..Default::default()
        },
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/usage")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let raw = resp.into_body().collect().await.unwrap().to_bytes();
    let body_string = String::from_utf8_lossy(&raw);
    let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let interactive = body["interactive"].as_array().expect("interactive array");
    let record = interactive
        .iter()
        .find(|r| {
            r.get("agent").and_then(|v| v.as_str()) == Some("gemini")
                && r.get("session_id").and_then(|v| v.as_str()) == Some("ralphy-probe-p1p2p3p4p6")
        })
        .unwrap_or_else(|| panic!("no gemini record; got: {body_string}"));
    // `total - input`, not the bare `output` field — the arithmetic survives
    // the whole route, not just the scan's own unit test.
    assert_eq!(record["tokens"]["output"].as_u64(), Some(287), "{record}");
    assert_eq!(record["tokens"]["input"].as_u64(), Some(20637), "{record}");
    // ADR-0043 D10: the served record must carry the floor label, or the UI
    // has nothing to render `≥ n (lower bound)` from.
    assert_eq!(record["lower_bound"].as_bool(), Some(true), "{record}");
}
