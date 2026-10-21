//! `/api/usage` over the run records, and `/api/spend`.

use super::*;

#[tokio::test]
async fn api_usage_serves_run_records_and_honors_since() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("owner-repo.jsonl"),
        "{\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"plan\",\"agent\":\"a\",\"model\":\"m\",\"session_id\":\"sess-a\",\"outcome\":\"ok\",\"tokens\":{\"input\":10,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
         {\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"m\",\"session_id\":\"sess-b\",\"outcome\":\"ok\",\"tokens\":{\"input\":20,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
    )
    .unwrap();

    let id = identity::Identity {
        id: ulid::Ulid::nil(),
        name: "anvil".into(),
        avatar: "🐙".into(),
    };
    let resp = router(
        Some(id),
        PathBuf::from("does-not-exist"),
        dir.path().to_path_buf(),
        StorePaths::default(),
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
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("\"sess-a\""),
        "must carry sess-a; got: {body}"
    );
    assert!(
        body.contains("\"sess-b\""),
        "must carry sess-b; got: {body}"
    );
    assert!(
        body.contains("00000000000000000000000000"),
        "must carry the daemon_id; got: {body}"
    );
    assert!(
        !body.contains("usd"),
        "must not carry a usd field; got: {body}"
    );
    assert!(
        !body.contains("cost"),
        "must not carry a cost field; got: {body}"
    );

    let id = identity::Identity {
        id: ulid::Ulid::nil(),
        name: "anvil".into(),
        avatar: "🐙".into(),
    };
    let resp = router(
        Some(id),
        PathBuf::from("does-not-exist"),
        dir.path().to_path_buf(),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/usage?since=2026-06-15T12:05:00%2B00:00")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("\"sess-b\"") && !body.contains("\"sess-a\""),
        "since must keep only sess-b; got: {body}"
    );
}

/// `/api/spend` serves the project's priced summary, and serves it SMALL: the
/// document is a fixed set of fields, so opening the tab never transfers the
/// ledger. The two ledger lines below are for different projects, so the
/// response also proves the route scopes on the one it was asked for.
///
/// The assertions are on SHAPE and on the floor marker, not on a dollar
/// figure: `PriceTable::load()` reads the host's own `pricing.toml`, and
/// pinning an amount here would make the test fail on an operator's machine
/// for doing exactly what that file is for.
#[tokio::test]
async fn api_spend_serves_one_projects_priced_summary_without_the_ledger() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("owner-repo.jsonl"),
        "{\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"unknown\",\"outcome\":\"ok\",\"tokens\":{\"input\":700,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
         {\"project\":\"other/repo\",\"issue\":2,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"claude-opus-4-8\",\"session_id\":\"sess-b\",\"outcome\":\"ok\",\"tokens\":{\"input\":50000,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
    )
    .unwrap();

    let resp = router(
        None,
        PathBuf::from("does-not-exist"),
        dir.path().to_path_buf(),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/spend?project=owner%2Frepo")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);

    let summary: spend::SpendSummary = serde_json::from_str(&body).expect("a spend document");
    assert_eq!(summary.project, "owner/repo");
    assert_eq!(
        summary.tokens.total, 700,
        "the other project's 50k must not be in it; got: {body}"
    );
    assert_eq!(summary.tokens.meter, "↑700 ⚡0 ❄0 ↓0");
    // `unknown` with no session_id: unpriceable and unrecoverable.
    assert!(
        summary.floor,
        "an unpriceable line makes it a floor: {body}"
    );
    assert_eq!(summary.total, "~$?", "never `$0`; got: {body}");
    assert_eq!(
        summary
            .unpriced
            .causes
            .iter()
            .map(|c| (c.key.as_str(), c.tokens))
            .collect::<Vec<_>>(),
        [("lost", 700)],
        "the only cause with volume is the one the line has; got: {body}"
    );
    assert!(
        !body.contains("\"ts\"") && !body.contains("\"phase\""),
        "the summary must not ship ledger rows; got: {body}"
    );
}

/// The Ledger grid's feed: `?project=` narrows the raw usage dump to one
/// project AND hands each surviving row its unpriced verdict, so the grid's
/// "unpriced only" filter never has to re-derive in JavaScript what the
/// price table already decided.
#[tokio::test]
async fn api_usage_scopes_to_a_project_and_marks_unpriced_rows() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("owner-repo.jsonl"),
        "{\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"unknown\",\"session_id\":\"sess-a\",\"outcome\":\"ok\",\"tokens\":{\"input\":700,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:00:00+00:00\"}\n\
         {\"project\":\"other/repo\",\"issue\":2,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"unknown\",\"outcome\":\"ok\",\"tokens\":{\"input\":50000,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:05:00+00:00\"}\n",
    )
    .unwrap();

    let resp = router(
        None,
        PathBuf::from("does-not-exist"),
        dir.path().to_path_buf(),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/usage?project=owner%2Frepo")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);

    assert_eq!(
        body.matches("other/repo").count(),
        0,
        "another project's rows must not reach the grid; got: {body}"
    );
    assert!(
        body.contains("\"sess-a\""),
        "the open project's row must survive; got: {body}"
    );
    assert!(
        body.contains("\"unpriced_cause\":\"recoverable\""),
        "an `unknown` model WITH a session is recoverable, not lost; got: {body}"
    );
}

/// The route is scoped to ONE project by construction: `project` is a required
/// query field, so a caller that forgets it gets a refusal rather than a
/// silently cross-project total.
#[tokio::test]
async fn api_spend_refuses_a_request_with_no_project() {
    let dir = tempfile::tempdir().unwrap();
    let resp = router(
        None,
        PathBuf::from("does-not-exist"),
        dir.path().to_path_buf(),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/spend")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// The period is a CLOSED vocabulary at the boundary: a key the surface does
/// not offer is refused, never quietly served as all-time figures under a
/// window label. The recognized key answers `200` and echoes itself, so the
/// client renders the window it is actually reading.
#[tokio::test]
async fn api_spend_refuses_an_unknown_period() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("owner-repo.jsonl"),
        "{\"project\":\"owner/repo\",\"issue\":1,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"unknown\",\"outcome\":\"ok\",\"tokens\":{\"input\":700,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"2026-06-15T12:00:00+00:00\"}\n",
    )
    .unwrap();
    let spend_get = |uri: &'static str| {
        let path = dir.path().to_path_buf();
        async move {
            router(
                None,
                PathBuf::from("does-not-exist"),
                path,
                StorePaths::default(),
                Instant::now(),
                idle_shutdown(),
                auth::AuthState::localhost(),
            )
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
        }
    };

    let refused = spend_get("/api/spend?project=owner%2Frepo&period=fortnight").await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);

    let accepted = spend_get("/api/spend?project=owner%2Frepo&period=7d").await;
    assert_eq!(accepted.status(), StatusCode::OK);
    let body = accepted.into_body().collect().await.unwrap().to_bytes();
    let summary: spend::SpendSummary = serde_json::from_slice(&body).expect("a spend document");
    assert_eq!(summary.period.key, "7d");
    assert_eq!(summary.period.label, "last 7 days");
    assert!(
        summary.period.since.is_some(),
        "a bounded window carries the instant its figures start at"
    );
    // The fixture row is from June: a 7-day window must not contain it.
    assert_eq!(summary.tokens.total, 0, "the window scopes the figures");
}
