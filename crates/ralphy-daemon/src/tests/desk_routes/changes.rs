use super::*;

#[tokio::test]
async fn api_desk_put_then_get_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let res = desk_put(
        dir.path(),
        serde_json::json!([
            create("window", &window_json("w-a", &serde_json::json!(7), true)),
            create(
                "window",
                &window_json("w-b", &serde_json::Value::Null, false)
            ),
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
            &window_json("w-1", &serde_json::json!(7), false)
        )]),
    )
    .await;
    let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();
    let legacy = serde_json::json!({
        "windows": [window_json("w-old", &serde_json::Value::Null, false)],
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
            create("note", &off),
            create("note", &bad_tree),
            create("note", &note_json("n-ok", "c.note")),
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
        .map(|n| create("fence", &fence_json(&format!("f{n}"), "region")))
        .collect();
    let notes: Vec<serde_json::Value> = (1..=desk::NOTE_MAX + 1)
        .map(|n| create("note", &note_json(&format!("n{n}"), &format!("a{n}.note"))))
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

/// One PUT, then what the next GET serves; `then` is a second PUT that clears or
/// refuses. The wire holds placement only, and an off or empty key is absent.
struct Trip {
    name: &'static str,
    first: serde_json::Value,
    put_has: Vec<&'static str>,
    get_has: Vec<&'static str>,
    get_absent: Vec<&'static str>,
    get_counts: Vec<(&'static str, usize)>,
    toml_counts: Vec<(&'static str, usize)>,
    then: serde_json::Value,
    then_absent: Vec<&'static str>,
    then_leaves_toml: bool,
}

fn trips() -> Vec<Trip> {
    let none = || serde_json::Value::Null;
    let mut with_checkout = window_json("w1", &none(), false);
    with_checkout["checkout"] = serde_json::json!("wt-a");
    let locked = |v: bool| serde_json::json!({ "locked": v });
    let base = || Trip {
        name: "",
        first: serde_json::json!([]),
        put_has: vec![],
        get_has: vec![],
        get_absent: vec![],
        get_counts: vec![],
        toml_counts: vec![],
        then: serde_json::json!([]),
        then_absent: vec![],
        then_leaves_toml: false,
    };
    vec![
        Trip {
            name: "notes",
            first: serde_json::json!([create("note", &note_json("n1", ".ralphy/notes/a.note"))]),
            get_has: vec![r#""id":"n1""#, r#""path":".ralphy/notes/a.note""#],
            get_absent: vec!["markdown", "color"],
            ..base()
        },
        Trip {
            name: "checkouts",
            first: serde_json::json!([
                { "op": "checkout", "repo": "owner/repo", "name": "wt-a" }
            ]),
            put_has: vec![r#""checkouts":{"owner/repo":"wt-a"}"#],
            get_has: vec![r#""checkouts":{"owner/repo":"wt-a"}"#],
            then: serde_json::json!([
                { "op": "checkout-clear", "repo": "owner/repo", "ifName": "wt-a" }
            ]),
            then_absent: vec!["checkouts"],
            ..base()
        },
        Trip {
            name: "a record's checkout, gated by the name check",
            first: serde_json::json!([create("window", &with_checkout)]),
            get_has: vec![r#""checkout":"wt-a""#],
            then: serde_json::json!([set(
                "window",
                "w1",
                &serde_json::json!({ "checkout": "../x" })
            )]),
            then_leaves_toml: true,
            ..base()
        },
        Trip {
            name: "a lock on a record and a fence",
            first: serde_json::json!([
                create("window", &window_json("w1", &none(), false)),
                create("fence", &fence_json("f1", "backend")),
                set("window", "w1", &locked(true)),
                set("fence", "f1", &locked(true)),
            ]),
            get_counts: vec![(r#""locked":true"#, 2)],
            toml_counts: vec![("locked = true", 2)],
            then: serde_json::json!([
                set("window", "w1", &locked(false)),
                set("fence", "f1", &locked(false)),
            ]),
            then_absent: vec!["locked"],
            ..base()
        },
    ]
}

#[tokio::test]
async fn api_desk_round_trips() {
    for t in trips() {
        let dir = tempfile::tempdir().unwrap();
        let res = desk_put(dir.path(), t.first).await;
        assert_eq!(res.status(), StatusCode::OK, "{}", t.name);
        let put_body = body_text(res).await;
        for needle in &t.put_has {
            assert!(put_body.contains(needle), "{}: {put_body}", t.name);
        }
        let get = desk_get(dir.path()).await;
        for needle in &t.get_has {
            assert!(get.contains(needle), "{}: {get}", t.name);
        }
        for needle in &t.get_absent {
            assert!(!get.contains(needle), "{}: {get}", t.name);
        }
        for (needle, n) in &t.get_counts {
            assert_eq!(get.matches(needle).count(), *n, "{}: {get}", t.name);
        }
        let toml_path = dir.path().join("desk.toml");
        let toml = std::fs::read_to_string(&toml_path).unwrap();
        for (needle, n) in &t.toml_counts {
            assert_eq!(toml.matches(needle).count(), *n, "{}: {toml}", t.name);
        }
        let res = desk_put(dir.path(), t.then).await;
        assert_eq!(res.status(), StatusCode::OK, "{}", t.name);
        let get = desk_get(dir.path()).await;
        for needle in &t.then_absent {
            assert!(!get.contains(needle), "{}: {get}", t.name);
        }
        if t.then_leaves_toml {
            let after = std::fs::read_to_string(&toml_path).unwrap();
            assert_eq!(
                after, toml,
                "{}: a refused change never reaches the store",
                t.name
            );
        }
    }
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
            create(
                "window",
                &window_json("w-a", &serde_json::Value::Null, false)
            ),
            create("fence", &fence_json("f-a", "backend")),
        ]),
    )
    .await;
    let before = std::fs::read_to_string(dir.path().join("desk.toml")).unwrap();

    let huge = create("fence", &fence_json("f-huge", "planning"))
        .to_string()
        .replace("\"left\":40.0", "\"left\":1e400");
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    let bad = StatusCode::BAD_REQUEST;
    // (case, raw body, expected status)
    let rows: [(&str, String, StatusCode); 9] = [
        ("not JSON", "{".into(), bad),
        (
            "the pre-#340 bare array",
            serde_json::json!([window_json("w-b", &serde_json::Value::Null, false)]).to_string(),
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
