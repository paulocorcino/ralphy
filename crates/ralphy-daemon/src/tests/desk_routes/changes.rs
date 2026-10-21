use super::*;

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
