use super::*;

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
