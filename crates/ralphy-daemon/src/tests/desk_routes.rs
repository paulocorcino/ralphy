//! `GET` / `PUT /api/desk` and `POST /api/desk/new`: the desk read, the
//! desk change list (ADR-0050 amendment 2026-10-04, changes, not the desk),
//! and the new desk after an unreadable one (ADR-0070 D4).

use super::*;

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

#[tokio::test]
async fn api_desk_empty_when_no_file() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        desk_get(dir.path()).await,
        r#"{"windows":[],"fences":[],"notes":[]}"#
    );
    assert!(
        !dir.path().join("desk.toml").exists(),
        "a GET must not create the store"
    );
}

/// ADR-0070 D4: a `desk.toml` that cannot be parsed is a failure on both
/// verbs, and the PUT never writes over it.
#[tokio::test]
async fn api_desk_refuses_a_corrupt_desk_and_leaves_its_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("desk.toml");
    let corrupt: &[u8] = b"windows = [\n";
    std::fs::write(&file, corrupt).unwrap();

    let res = desk_router(dir.path())
        .oneshot(
            Request::builder()
                .uri("/api/desk")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let body = reply_json(res).await;
    assert_eq!(body["state"], "unreadable");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("parsing desk layout"),
        "the reply says why: {body}"
    );

    let res = desk_put(
        dir.path(),
        serde_json::json!([create(
            "window",
            window_json("w-a", serde_json::json!(7), false)
        )]),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(
        std::fs::read(&file).unwrap(),
        corrupt,
        "a refused PUT leaves the unreadable desk byte-identical"
    );
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

/// ADR-0070 D4: the operator's one action on an unreadable desk keeps the old
/// file under a new name and starts an empty desk. The new desk has a
/// generation, so a page that read the unreadable one reloads before it
/// writes.
#[tokio::test]
async fn api_desk_new_moves_the_unreadable_file_aside() {
    let dir = tempfile::tempdir().unwrap();
    let corrupt: &[u8] = b"windows = [\n";
    std::fs::write(dir.path().join("desk.toml"), corrupt).unwrap();

    let res = desk_new(dir.path()).await;
    assert_eq!(res.status(), StatusCode::OK);
    let copies = unreadable_copies(dir.path());
    assert_eq!(copies.len(), 1, "one aside copy: {copies:?}");
    assert_eq!(std::fs::read(&copies[0]).unwrap(), corrupt);
    let fresh: desk::DeskStore = serde_json::from_str(&desk_get(dir.path()).await).unwrap();
    assert!(fresh.windows.is_empty() && fresh.fences.is_empty() && fresh.notes.is_empty());
    assert!(
        fresh.generation > 0,
        "a new desk has a generation: {fresh:?}"
    );

    let res = desk_put(dir.path(), serde_json::json!([])).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(
        reply_json(res).await["state"],
        "restored",
        "a page from before reloads"
    );
}

/// A `desk.toml` that cannot be READ (here a directory in its place) may be a
/// fine file held for a moment, so it is `unavailable`, not `unreadable`:
/// writes are refused, and starting a new desk moves nothing aside.
#[tokio::test]
async fn a_desk_that_cannot_be_read_is_unavailable_and_never_moved_aside() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("desk.toml");
    std::fs::create_dir(&file).unwrap();

    let res = desk_router(dir.path())
        .oneshot(
            Request::builder()
                .uri("/api/desk")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply_json(res).await["state"], "unavailable");

    let res = desk_put(
        dir.path(),
        serde_json::json!([create(
            "window",
            window_json("w-a", serde_json::json!(7), false)
        )]),
    )
    .await;
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);

    let res = desk_new(dir.path()).await;
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        unreadable_copies(dir.path()).is_empty(),
        "nothing is moved aside"
    );
    assert!(file.is_dir(), "the path is left as it was");
}

/// Negative control: a desk that reads is never moved aside.
#[tokio::test]
async fn api_desk_new_refuses_a_readable_desk() {
    let dir = tempfile::tempdir().unwrap();
    let res = desk_put(
        dir.path(),
        serde_json::json!([create(
            "window",
            window_json("w-a", serde_json::json!(7), false)
        )]),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let res = desk_new(dir.path()).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert!(unreadable_copies(dir.path()).is_empty());
}

#[tokio::test]
async fn api_desk_put_then_get_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let res = desk_put(
        dir.path(),
        serde_json::json!([
            create("window", window_json("w-a", serde_json::json!(7), true)),
            create("window", window_json("w-b", serde_json::Value::Null, false)),
        ]),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    let body = desk_get(dir.path()).await;
    assert!(
        body.contains("\"sessionId\":7"),
        "camelCase wire key: {body}"
    );
    assert!(body.contains("\"max\":true"), "maximized survives: {body}");
    let a = body.find("w-a").expect("first record present");
    let b = body.find("w-b").expect("second record present");
    assert!(a < b, "layout order is creation order: {body}");
}

/// A page from before the change list sends whole records. Its body is
/// refused with the state a current page reloads on, and nothing is written.
#[tokio::test]
async fn a_body_without_changes_is_refused_as_restored_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    desk_put(
        dir.path(),
        serde_json::json!([create(
            "window",
            window_json("w-1", serde_json::json!(7), false)
        )]),
    )
    .await;
    let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();
    let legacy = serde_json::json!({
        "windows": [window_json("w-old", serde_json::Value::Null, false)],
        "fences": [],
        "removed": { "windows": ["w-1"] },
    });
    let res = desk_put_raw(dir.path(), legacy.to_string()).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(reply_json(res).await["state"], "restored");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
        before,
        "an old page's body never reaches the store"
    );
}

/// A refused change is named in the reply and writes nothing; the valid
/// changes of the same body still apply.
#[tokio::test]
async fn a_refused_change_is_named_and_the_rest_of_the_body_applies() {
    let dir = tempfile::tempdir().unwrap();
    let mut off = note_json("n-huge", "a.note");
    off["rect"]["top"] = serde_json::json!(-1.0);
    let mut bad_tree = note_json("n-bad", "b.note");
    bad_tree["checkout"] = serde_json::json!("../escape");
    let res = desk_put(
        dir.path(),
        serde_json::json!([
            create("note", off),
            create("note", bad_tree),
            create("note", note_json("n-ok", "c.note")),
        ]),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let reply = reply_json(res).await;
    let refused = reply["refused"].as_array().expect("refused is a list");
    assert_eq!(refused.len(), 2, "{reply}");
    assert_eq!(refused[0]["index"], 0);
    assert!(refused[0]["error"]
        .as_str()
        .unwrap()
        .contains("note n-huge has an out-of-frame rect"));
    assert_eq!(refused[1]["index"], 1);
    assert!(refused[1]["error"]
        .as_str()
        .unwrap()
        .contains("checkout ../escape on record n-bad is not a valid name"));
    let ids: Vec<String> = desk_on_disk(dir.path())
        .notes
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(ids, ["n-ok"]);
}

/// Each cap holds on `create`: the store stops at the cap and keeps the first
/// records. No record is ever pruned by age on this path.
#[tokio::test]
async fn a_create_past_a_cap_is_refused_and_nothing_is_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let fences: Vec<serde_json::Value> = (1..=desk::FENCE_MAX + 1)
        .map(|n| create("fence", fence_json(&format!("f{n}"), "region")))
        .collect();
    let notes: Vec<serde_json::Value> = (1..=desk::NOTE_MAX + 1)
        .map(|n| create("note", note_json(&format!("n{n}"), &format!("a{n}.note"))))
        .collect();
    let all: Vec<serde_json::Value> = fences.into_iter().chain(notes).collect();
    let res = desk_put(dir.path(), serde_json::json!(all)).await;
    assert_eq!(res.status(), StatusCode::OK);
    let stored = desk_on_disk(dir.path());
    assert_eq!(stored.fences.len(), desk::FENCE_MAX);
    assert_eq!(stored.fences[0].id, "f1", "the first fence is never pruned");
    assert_eq!(stored.notes.len(), desk::NOTE_MAX);
    assert_eq!(stored.notes[0].id, "n1");
}

/// The third collection travels the same route as the other two.
#[tokio::test]
async fn api_desk_round_trips_notes() {
    let dir = tempfile::tempdir().unwrap();
    let res = desk_put(
        dir.path(),
        serde_json::json!([create("note", note_json("n1", ".ralphy/notes/a.note"))]),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    let served = desk_get(dir.path()).await;
    assert!(served.contains(r#""id":"n1""#), "{served}");
    assert!(
        served.contains(r#""path":".ralphy/notes/a.note""#),
        "{served}"
    );
    // Placement only: the wire record carries no text and no colour.
    assert!(!served.contains("markdown"), "{served}");
    assert!(!served.contains("color"), "{served}");
}

/// ADR-0063 §4: the selected checkout per repo ref, selected and cleared by
/// its own changes, answered on the PUT and served on the next GET.
#[tokio::test]
async fn api_desk_round_trips_checkouts() {
    let dir = tempfile::tempdir().unwrap();
    let res = desk_put(
        dir.path(),
        serde_json::json!([{ "op": "checkout", "repo": "owner/repo", "name": "wt-a" }]),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let put_body = body_text(res).await;
    assert!(
        put_body.contains(r#""checkouts":{"owner/repo":"wt-a"}"#),
        "the PUT answers the checkouts: {put_body}"
    );
    let get_body = desk_get(dir.path()).await;
    assert!(
        get_body.contains(r#""checkouts":{"owner/repo":"wt-a"}"#),
        "the GET serves them: {get_body}"
    );

    // Clearing the selection drops the key from the wire body entirely —
    // an empty map is not serialised, so the old exact shape holds.
    let res = desk_put(
        dir.path(),
        serde_json::json!([{ "op": "checkout-clear", "repo": "owner/repo", "ifName": "wt-a" }]),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let get_body = desk_get(dir.path()).await;
    assert!(!get_body.contains("checkouts"), "{get_body}");
}

/// A registry whose `owner/repo` entry lists `path-abc` as a former slug —
/// what the CLI's migration leaves behind after a gained remote.
fn registry_with_former_slug(dir: &Path) {
    let mut store = registry::RegistryStore::default();
    store.upsert("path-abc", "/repo");
    store.rekey("path-abc", "owner/repo");
    registry::save_to(&store, &dir.join("repos.toml")).unwrap();
}

/// A desk saved BEFORE a re-key still names the former slug on disk; the
/// GET serves it under the canonical key so the migrated project's
/// consoles come back to it (ADR-0036 amendment 2026-09-16).
#[tokio::test]
async fn desk_get_serves_a_former_slug_as_its_canonical_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut stale = desk::DeskStore::default();
    stale.windows.push(desk::DeskRecord {
        id: "w1".into(),
        repo: "path-abc".into(),
        rect: desk::DeskRect {
            left: 1.0,
            top: 1.0,
            width: 300.0,
            height: 200.0,
        },
        ..desk::DeskRecord::default()
    });
    stale.checkouts.insert("path-abc".into(), "wt-a".into());
    desk::save_to(&stale, &dir.path().join("desk.toml")).unwrap();
    registry_with_former_slug(dir.path());

    let body = desk_get(dir.path()).await;
    assert!(
        body.contains(r#""repo":"owner/repo""#) && !body.contains("path-abc"),
        "the record follows the key: {body}"
    );
    assert!(
        body.contains(r#""checkouts":{"owner/repo":"wt-a"}"#),
        "the selection follows the key: {body}"
    );
}

/// A tab that read the desk before the re-key sends the former slug in its
/// changes; the PUT maps it, so `desk.toml` converges on the first save
/// whichever tab saves — and never regresses to the hash.
#[tokio::test]
async fn desk_put_maps_a_former_slug_in_a_change() {
    let dir = tempfile::tempdir().unwrap();
    registry_with_former_slug(dir.path());
    let mut stale = window_json("w1", serde_json::Value::Null, false);
    stale["repo"] = serde_json::json!("path-abc");
    let res = desk_put(
        dir.path(),
        serde_json::json!([
            create("window", stale),
            { "op": "checkout", "repo": "path-abc", "name": "wt-a" },
        ]),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let put_body = body_text(res).await;
    assert!(
        put_body.contains(r#""repo":"owner/repo""#) && !put_body.contains("path-abc"),
        "the answer is the canonical truth: {put_body}"
    );
    assert!(
        put_body.contains(r#""checkouts":{"owner/repo":"wt-a"}"#),
        "{put_body}"
    );
    let on_disk = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();
    assert!(
        !on_disk.contains("path-abc"),
        "the former slug never reaches the store: {on_disk}"
    );
}

/// `rev` rises with each write that changes the desk, and only then; both
/// verbs serve it.
#[tokio::test]
async fn rev_rises_only_when_a_write_changes_the_desk() {
    let dir = tempfile::tempdir().unwrap();
    let res = desk_put(
        dir.path(),
        serde_json::json!([create(
            "window",
            window_json("w-1", serde_json::json!(7), false)
        )]),
    )
    .await;
    assert_eq!(reply_json(res).await["rev"], 1);
    let noop = serde_json::json!([set("window", "w-1", serde_json::json!({ "max": false }))]);
    let res = desk_put(dir.path(), noop).await;
    assert_eq!(reply_json(res).await["rev"], 1, "a no-op leaves rev");
    let res = desk_put(
        dir.path(),
        serde_json::json!([set("window", "w-1", serde_json::json!({ "max": true }))]),
    )
    .await;
    assert_eq!(reply_json(res).await["rev"], 2);
    let served: serde_json::Value = serde_json::from_str(&desk_get(dir.path()).await).unwrap();
    assert_eq!(served["rev"], 2, "the GET serves it");
}

/// A resend of a batch the daemon already applied (the reply was lost) must
/// not undo a change another device made in between.
#[tokio::test]
async fn a_resend_with_an_old_seq_does_not_undo_a_later_change() {
    let dir = tempfile::tempdir().unwrap();
    desk_put(
        dir.path(),
        serde_json::json!([create(
            "window",
            window_json("w-1", serde_json::json!(7), false)
        )]),
    )
    .await;
    let to = |left: f64| {
        serde_json::json!([set(
            "window",
            "w-1",
            serde_json::json!({ "rect": rect_at(left) })
        )])
    };
    // The phone's batch lands; its reply is lost.
    let res = tab_put(dir.path(), "seq-phone", 4, to(400.0)).await;
    assert_eq!(res.status(), StatusCode::OK);
    // The PC moves the window again.
    tab_put(dir.path(), "seq-pc", 1, to(200.0)).await;
    // The phone resends the same batch, with the same number.
    let res = tab_put(dir.path(), "seq-phone", 4, to(400.0)).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        reply_json(res).await["windows"][0]["rect"]["left"],
        200.0,
        "the reply is the current desk"
    );
    assert_eq!(desk_on_disk(dir.path()).windows[0].rect.left, 200.0);
    // A higher number from the same tab applies.
    tab_put(dir.path(), "seq-phone", 5, to(300.0)).await;
    assert_eq!(desk_on_disk(dir.path()).windows[0].rect.left, 300.0);
}

/// #411: a record's own `checkout` round-trips and is gated by the same name
/// check as the selection map.
#[tokio::test]
async fn api_desk_round_trips_and_gates_a_records_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let mut record = window_json("w1", serde_json::Value::Null, false);
    record["checkout"] = serde_json::json!("wt-a");
    let res = desk_put(dir.path(), serde_json::json!([create("window", record)])).await;
    assert_eq!(res.status(), StatusCode::OK);
    let get_body = desk_get(dir.path()).await;
    assert!(
        get_body.contains(r#""checkout":"wt-a""#),
        "the GET serves the record's checkout: {get_body}"
    );
    let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();
    let res = desk_put(
        dir.path(),
        serde_json::json!([set(
            "window",
            "w1",
            serde_json::json!({ "checkout": "../x" })
        )]),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
        before,
        "a refused record checkout never reaches the store"
    );
}

/// Lock amendment (ADR-0050/0051, 2026-09-20): a locked window and a locked
/// fence round-trip through the route, and an unlock clears the key from the
/// wire again.
#[tokio::test]
async fn api_desk_round_trips_a_lock_on_a_record_and_a_fence() {
    let dir = tempfile::tempdir().unwrap();
    let res = desk_put(
        dir.path(),
        serde_json::json!([
            create("window", window_json("w1", serde_json::Value::Null, false)),
            create("fence", fence_json("f1", "backend")),
            set("window", "w1", serde_json::json!({ "locked": true })),
            set("fence", "f1", serde_json::json!({ "locked": true })),
        ]),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let get_body = desk_get(dir.path()).await;
    assert_eq!(
        get_body.matches(r#""locked":true"#).count(),
        2,
        "the GET serves both locks: {get_body}"
    );
    let toml = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();
    assert_eq!(toml.matches("locked = true").count(), 2, "toml={toml}");
    let res = desk_put(
        dir.path(),
        serde_json::json!([
            set("window", "w1", serde_json::json!({ "locked": false })),
            set("fence", "f1", serde_json::json!({ "locked": false })),
        ]),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let get_body = desk_get(dir.path()).await;
    assert!(
        !get_body.contains("locked"),
        "an unlocked desk carries no key: {get_body}"
    );
}

/// A checkout value that is not one path component is refused before any
/// write — the desk is the one place a name is stored, so a traversal must
/// never be persisted for a later verb to prefix.
#[tokio::test]
async fn api_desk_refuses_a_malformed_checkout_name() {
    let dir = tempfile::tempdir().unwrap();
    desk_put(
        dir.path(),
        serde_json::json!([{ "op": "checkout", "repo": "owner/repo", "name": "wt-a" }]),
    )
    .await;
    let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

    for bad in ["a/b", "../x", "", "a\\b", "."] {
        let res = desk_put(
            dir.path(),
            serde_json::json!([{ "op": "checkout", "repo": "owner/repo", "name": bad }]),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK, "checkout {bad:?}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "a refused checkout {bad:?} never reaches the store"
        );
    }
}

/// A body that is not `{ seq, changes }` never reaches the store: the
/// operator's desk survives byte for byte. The sequences each defeat a
/// different half-fix of #340: `[]`, `[[],[]]` and `[1,0,[]]` could satisfy
/// the struct POSITIONALLY. (An object without `changes` is an old page:
/// `a_body_without_changes_is_refused_as_restored_and_writes_nothing`.) An
/// out-of-range float literal is spelled in the RAW
/// body, because `json!(f64::INFINITY)` becomes `null`: `serde_json` refuses
/// it as a syntax error, so it is `400`.
#[tokio::test]
async fn api_desk_put_refuses_a_bad_body_without_touching_the_store() {
    let dir = tempfile::tempdir().unwrap();
    desk_put(
        dir.path(),
        serde_json::json!([
            create("window", window_json("w-a", serde_json::Value::Null, false)),
            create("fence", fence_json("f-a", "backend")),
        ]),
    )
    .await;
    let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

    let huge = create("fence", fence_json("f-huge", "planning"))
        .to_string()
        .replace("\"left\":40.0", "\"left\":1e400");
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    let bad = StatusCode::BAD_REQUEST;
    // (case, raw body, expected status)
    let rows: [(&str, String, StatusCode); 9] = [
        ("not JSON", "{".into(), bad),
        (
            "the pre-#340 bare array",
            serde_json::json!([window_json("w-b", serde_json::Value::Null, false)]).to_string(),
            unprocessable,
        ),
        ("`[]`", "[]".into(), unprocessable),
        ("`[[],[]]`", "[[],[]]".into(), unprocessable),
        ("`[1,0,[]]`", "[1,0,[]]".into(), unprocessable),
        ("no seq", r#"{"changes":[]}"#.into(), unprocessable),
        (
            "changes not a list",
            r#"{"seq":1,"changes":{}}"#.into(),
            unprocessable,
        ),
        (
            "an old field beside changes",
            r#"{"seq":1,"changes":[],"windows":[]}"#.into(),
            unprocessable,
        ),
        (
            "a non-finite rect",
            format!(r#"{{"seq":1,"changes":[{huge}]}}"#),
            bad,
        ),
    ];
    for (case, body, want) in rows {
        let res = desk_put_raw(dir.path(), body).await;
        assert_eq!(res.status(), want, "{case}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("desk.toml")).unwrap(),
            before,
            "{case}: a rejected upload never reaches the store"
        );
    }
}
