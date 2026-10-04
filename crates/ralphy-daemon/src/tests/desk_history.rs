//! `/api/desk/history` and the `generation` guard on `PUT /api/desk` (ADR-0050
//! amendment 2026-10-04, desk history).

use super::*;
use crate::desk::history::{self, Reason};

async fn call(
    dir: &Path,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let res = desk_router(dir)
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let text = body_text(res).await;
    (
        status,
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
    )
}

fn window_at(id: &str, left: f64, session: Option<u64>) -> desk::DeskRecord {
    desk::DeskRecord {
        id: id.into(),
        repo: "owner/repo".into(),
        agent: "claude".into(),
        kind: "agent".into(),
        rect: desk::DeskRect {
            left,
            top: 20.0,
            width: 640.0,
            height: 480.0,
        },
        session_id: session,
        ts: 1,
        ..Default::default()
    }
}

fn one_window(left: f64) -> desk::DeskStore {
    desk::DeskStore {
        windows: vec![window_at("w1", left, Some(1))],
        ..Default::default()
    }
}

fn reasons(rows: &serde_json::Value) -> Vec<String> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| r["reason"].as_str().unwrap().to_string())
        .collect()
}

fn window_change(session: u64) -> serde_json::Value {
    serde_json::json!({
        "op": "create", "type": "window",
        "record": {
            "id": "w1", "repo": "owner/repo", "agent": "claude", "kind": "console",
            "rect": { "left": 10.0, "top": 20.0, "width": 640.0, "height": 480.0 },
            "max": false, "sessionId": session,
        },
    })
}

async fn put_changes(
    dir: &Path,
    generation: u64,
    changes: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({ "seq": 1, "generation": generation, "changes": changes });
    call(dir, "PUT", "/api/desk", Some(body)).await
}

#[tokio::test]
async fn a_put_from_a_page_that_read_before_a_restore_is_refused_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("desk.toml");
    let mut stored = one_window(300.0);
    stored.generation = 5;
    desk::save_to(&stored, &file).unwrap();
    let bytes = std::fs::read(&file).unwrap();

    let moved = serde_json::json!([{
        "op": "set", "type": "window", "id": "w1",
        "fields": { "rect": { "left": 99.0, "top": 20.0, "width": 640.0, "height": 480.0 } },
    }]);
    let (status, reply) = put_changes(dir.path(), 4, moved.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(reply["state"], "restored");
    assert_eq!(std::fs::read(&file).unwrap(), bytes, "nothing was written");

    let (status, _) = put_changes(dir.path(), 5, moved).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_put_that_moves_a_window_writes_a_version_and_a_reconnect_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let (status, _) = put_changes(dir.path(), 0, serde_json::json!([window_change(1)])).await;
    assert_eq!(status, StatusCode::OK);
    let (_, rows) = call(dir.path(), "GET", "/api/desk/history", None).await;
    assert_eq!(reasons(&rows), vec!["change"]);

    let reconnect = serde_json::json!([{
        "op": "set", "type": "window", "id": "w1",
        "fields": { "session": { "sessionId": 8 } },
    }]);
    let (status, _) = put_changes(dir.path(), 0, reconnect).await;
    assert_eq!(status, StatusCode::OK);
    let (_, rows) = call(dir.path(), "GET", "/api/desk/history", None).await;
    assert_eq!(
        rows.as_array().unwrap().len(),
        1,
        "a new ts and session are not a change"
    );
}

#[tokio::test]
async fn a_restore_saves_the_desk_before_it_and_raises_the_generation() {
    let dir = tempfile::tempdir().unwrap();
    let history_dir = dir.path().join("desk-history");
    let id = history::append(&history_dir, &one_window(300.0), Reason::Change, 1_000).unwrap();
    desk::save_to(&one_window(10.0), &dir.path().join("desk.toml")).unwrap();

    let (status, reply) = call(
        dir.path(),
        "POST",
        "/api/desk/history",
        Some(serde_json::json!({ "id": id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let generation = reply["generation"].as_u64().unwrap();
    assert!(generation > 0);

    let (_, now) = call(dir.path(), "GET", "/api/desk", None).await;
    assert_eq!(now["generation"].as_u64(), Some(generation));
    assert_eq!(now["rev"], 1, "a restore raises rev");
    assert_eq!(now["windows"][0]["rect"]["left"], 300.0);
    assert_eq!(
        now["windows"][0]["sessionId"], 1,
        "the running session is kept"
    );

    let (_, rows) = call(dir.path(), "GET", "/api/desk/history", None).await;
    assert_eq!(reasons(&rows), vec!["restore", "before-restore", "change"]);
    let before_id = rows[1]["id"].as_i64().unwrap();
    let (status, before) = call(
        dir.path(),
        "GET",
        &format!("/api/desk/history?id={before_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(before["kind"], history::VERSION_KIND);
    assert_eq!(before["desk"]["windows"][0]["rect"]["left"], 10.0);
}

#[tokio::test]
async fn an_upload_a_put_would_refuse_is_refused_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut bad = serde_json::to_value(history::Version {
        kind: history::VERSION_KIND.into(),
        id: 1,
        started_at: 1,
        saved_at: 1,
        reason: Reason::Change,
        desk: one_window(10.0),
    })
    .unwrap();
    bad["desk"]["windows"][0]["rect"]["left"] = serde_json::json!(-5.0);
    let (status, _) = call(
        dir.path(),
        "POST",
        "/api/desk/history",
        Some(serde_json::json!({ "version": bad })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mut not_a_version = bad.clone();
    not_a_version["kind"] = serde_json::json!("something-else");
    not_a_version["desk"]["windows"][0]["rect"]["left"] = serde_json::json!(5.0);
    let (status, _) = call(
        dir.path(),
        "POST",
        "/api/desk/history",
        Some(serde_json::json!({ "version": not_a_version })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    assert!(!dir.path().join("desk.toml").exists());
    assert!(!dir.path().join("desk-history").exists());
}

#[tokio::test]
async fn an_unknown_version_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (status, _) = call(dir.path(), "GET", "/api/desk/history?id=7", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        dir.path(),
        "POST",
        "/api/desk/history",
        Some(serde_json::json!({ "id": 7 })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!dir.path().join("desk.toml").exists());
}
