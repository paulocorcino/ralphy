use super::*;

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
