//! `GET` / `PUT /api/desk` and `POST /api/desk/new`: the desk read, the
//! desk change list (ADR-0050 amendment 2026-10-04, changes, not the desk),
//! and the new desk after an unreadable one (ADR-0070 D4).

use super::*;

mod changes;
mod new_desk;
mod slug_and_rev;

async fn desk_get(dir: &Path) -> String {
    let res = desk_router(dir)
        .oneshot(
            Request::builder()
                .uri("/api/desk")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    body_text(res).await
}

fn desk_on_disk(dir: &Path) -> desk::DeskStore {
    desk::load_from(&dir.join("desk.toml")).expect("desk reads")
}

/// PUT a RAW body to `uri` — the only way to send a shape the route must
/// refuse (a bare array, an out-of-range float literal).
async fn put_raw_to(dir: &Path, uri: &str, body: String) -> Response {
    desk_router(dir)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn desk_put_raw(dir: &Path, body: String) -> Response {
    put_raw_to(dir, "/api/desk", body).await
}

/// PUT `changes` with no tab, so no upload number is remembered.
async fn desk_put(dir: &Path, changes: serde_json::Value) -> Response {
    let body = serde_json::json!({ "seq": 1, "changes": changes });
    desk_put_raw(dir, body.to_string()).await
}

/// PUT `changes` as upload `seq` of `tab`.
async fn tab_put(dir: &Path, tab: &str, seq: u64, changes: serde_json::Value) -> Response {
    let body = serde_json::json!({ "seq": seq, "changes": changes });
    put_raw_to(dir, &format!("/api/desk?tab={tab}"), body.to_string()).await
}

async fn reply_json(res: Response) -> serde_json::Value {
    serde_json::from_str(&body_text(res).await).expect("a JSON reply")
}

fn window_json(id: &str, session_id: serde_json::Value, max: bool) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "repo": "owner/repo",
        "agent": "claude",
        "kind": "console",
        "rect": { "left": 10.0, "top": 20.0, "width": 640.0, "height": 480.0 },
        "max": max,
        "sessionId": session_id,
        "ts": 1,
    })
}

fn fence_json(id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "rect": { "left": 40.0, "top": 40.0, "width": 720.0, "height": 460.0 },
        "ts": 1,
    })
}

/// A note card on the wire (ADR-0064 §2): placement only.
fn note_json(id: &str, path: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "repo": "owner/repo",
        "path": path,
        "rect": { "left": 80.0, "top": 120.0, "width": 240.0, "height": 180.0 },
        "ts": 1,
    })
}

fn create(kind: &str, record: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "op": "create", "type": kind, "record": record })
}

fn set(kind: &str, id: &str, fields: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "op": "set", "type": kind, "id": id, "fields": fields })
}

fn rect_at(left: f64) -> serde_json::Value {
    serde_json::json!({ "left": left, "top": 20.0, "width": 640.0, "height": 480.0 })
}

async fn desk_new(dir: &Path) -> Response {
    desk_router(dir)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/desk/new")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn unreadable_copies(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("desk.toml.unreadable-"))
        })
        .collect()
}

/// A registry whose `owner/repo` entry lists `path-abc` as a former slug —
/// what the CLI's migration leaves behind after a gained remote.
fn registry_with_former_slug(dir: &Path) {
    let mut store = registry::RegistryStore::default();
    store.upsert("path-abc", "/repo");
    store.rekey("path-abc", "owner/repo");
    registry::save_to(&store, &dir.join("repos.toml")).unwrap();
}
