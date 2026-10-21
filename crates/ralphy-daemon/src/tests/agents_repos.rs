//! `/api/agents` and `/api/repos`.

use super::*;
use std::sync::Arc;

#[tokio::test]
async fn api_agents_serves_the_roster() {
    let resp = get("/api/agents").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(rows.len(), session::Agent::ALL.len());
    let served: std::collections::BTreeSet<String> = rows
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    let expected: std::collections::BTreeSet<String> = session::Agent::ALL
        .iter()
        .map(|&a| crate::dispatch::agent_flag(a).to_string())
        .collect();
    assert_eq!(served, expected, "served roster ids: {served:?}");
    assert_eq!(rows[0]["id"], "claude");
    assert_eq!(rows[0]["label"], "claude");
    assert_eq!(rows[0]["accelerator"], "1");
    assert!(rows[0]["available"].is_boolean());
    if rows[0]["available"] == false {
        assert_eq!(rows[0]["reason"], "not installed here");
    } else {
        assert!(rows[0]["reason"].is_null());
    }
}

#[tokio::test]
async fn api_agents_uses_the_owning_daemons_locator() {
    const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const PEER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
    let peer_store = tempfile::tempdir().unwrap();
    let peer_identity = identity::Identity {
        id: PEER_ID.parse().unwrap(),
        name: "peer".to_string(),
        avatar: "🐙".to_string(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let peer_port = listener.local_addr().unwrap().port();
    let (peer_shutdown, peer_rx) = tokio::sync::watch::channel(false);
    let peer_router = router_with_roster(
        Some(peer_identity),
        peer_store.path().join("repos.toml"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        peer_rx,
        RouterDependencies {
            auth: auth::AuthState::fixed(
                auth::AuthPolicy::Bearer("peer-token".to_string()),
                epoch::SessionEpoch::in_memory_detached(),
            ),
            roster_locator: Arc::new(|_| Some(PathBuf::from("/usr/local/bin/vendor"))),
        },
    );
    let peer_task = tokio::spawn(async move {
        let _shutdown = peer_shutdown;
        axum::serve(listener, peer_router).await.unwrap();
    });

    let local_store = tempfile::tempdir().unwrap();
    peer::write_descriptor(
        local_store.path(),
        &peer::PeerDescriptor {
            daemon_id: PEER_ID.to_string(),
            name: "peer".to_string(),
            avatar: "🐙".to_string(),
            address: "127.0.0.1".to_string(),
            port: peer_port,
            environment: "WSL: Ubuntu-22.04".to_string(),
            os: String::new(),
            token: "peer-token".to_string(),
            protocol_version: peer::PEER_PROTOCOL_VERSION,
            tunnel: None,
            nudge: None,
        },
    )
    .unwrap();
    let local_router = router_with_roster(
        Some(identity::Identity {
            id: LOCAL_ID.parse().unwrap(),
            name: "local".to_string(),
            avatar: "🐙".to_string(),
        }),
        local_store.path().join("repos.toml"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        RouterDependencies {
            auth: auth::AuthState::localhost(),
            roster_locator: Arc::new(|_| None),
        },
    );

    let local = local_router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let local_rows: serde_json::Value = serde_json::from_str(&body_text(local).await).unwrap();
    assert_eq!(local_rows[0]["available"], false);

    let peer = local_router
        .oneshot(
            Request::builder()
                .uri(format!("/api/agents?repo={PEER_ID}%2Fowner%2Fshared"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(peer.status(), StatusCode::OK);
    let peer_rows: serde_json::Value = serde_json::from_str(&body_text(peer).await).unwrap();
    assert_eq!(peer_rows[0]["available"], true);
    assert_eq!(peer_rows[0]["reason"], serde_json::Value::Null);
    peer_task.abort();
}

#[tokio::test]
async fn api_repos_reports_reachability_and_branch() {
    // One existing-dir entry on a branch (reachable) and one bogus-path entry
    // (unreachable), read back through the route.
    let dir = tempfile::tempdir().unwrap();
    // A real repo: the branch is git's answer, not a read of `.git/HEAD`.
    let init = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["init", "-q", "-b", "feat/mini-ide"])
        .output()
        .expect("git (CI and the build machine have git)");
    assert!(init.status.success(), "git init failed");
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/here", &dir.path().to_string_lossy());
    store.upsert("owner/gone", "/no/such/path/exists");
    registry::save_to(&store, &registry_path).unwrap();

    let resp = router(
        None,
        registry_path,
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/repos")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("owner/here") && body.contains("owner/gone"),
        "body must carry both slugs; got: {body}"
    );
    assert!(
        body.contains("\"name\":\"owner/here\""),
        "each row carries the project name; got: {body}"
    );
    assert!(
        body.contains("\"reachable\":true"),
        "the existing-dir entry must be reachable; got: {body}"
    );
    assert!(
        body.contains("\"reachable\":false"),
        "the bogus-path entry must be unreachable; got: {body}"
    );
    assert!(
        body.contains("\"branch\":\"feat/mini-ide\""),
        "the reachable repo's branch must be reported; got: {body}"
    );
    assert!(
        body.contains("\"branch\":null"),
        "the unreachable repo's branch must be null; got: {body}"
    );
}

#[tokio::test]
async fn api_repos_reports_dirty_and_remote() {
    fn git(dir: &std::path::Path, args: &[&str]) {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git (CI and the build machine have git)");
    }

    // (a) a dirty repo (untracked file) WITH an origin remote.
    let dirty = tempfile::tempdir().unwrap();
    git(dirty.path(), &["init"]);
    git(
        dirty.path(),
        &["remote", "add", "origin", "https://github.com/o/r.git"],
    );
    std::fs::write(dirty.path().join("untracked.txt"), "x").unwrap();
    // (b) a clean repo with NO remote.
    let clean = tempfile::tempdir().unwrap();
    git(clean.path(), &["init"]);
    // (c) a repo on a detached HEAD.
    let detached = tempfile::tempdir().unwrap();
    git(detached.path(), &["init", "-b", "main"]);
    git(
        detached.path(),
        &[
            "-c",
            "user.email=t@e",
            "-c",
            "user.name=T",
            "commit",
            "--allow-empty",
            "-m",
            "x",
        ],
    );
    git(detached.path(), &["checkout", "--detach"]);

    let reg = tempfile::tempdir().unwrap();
    let registry_path = reg.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/dirty", &dirty.path().to_string_lossy());
    store.upsert("owner/clean", &clean.path().to_string_lossy());
    store.upsert("owner/detached", &detached.path().to_string_lossy());
    registry::save_to(&store, &registry_path).unwrap();

    let resp = router(
        None,
        registry_path,
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(
        Request::builder()
            .uri("/api/repos")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("\"dirty\":true"),
        "the untracked-file repo must be dirty; got: {body}"
    );
    assert!(
        body.contains("\"dirty\":false"),
        "the clean repo must not be dirty; got: {body}"
    );
    assert!(
        body.contains("\"remote\":\"https://github.com/o/r.git\""),
        "the origin url must be reported; got: {body}"
    );
    assert!(
        body.contains("\"remote\":null"),
        "the remoteless repo must report null; got: {body}"
    );

    let rows: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
    let row = |slug: &str| {
        rows.iter()
            .find(|r| r["slug"] == slug)
            .unwrap_or_else(|| panic!("no row for {slug}: {body}"))
            .clone()
    };
    let detached = row("owner/detached");
    assert_eq!(detached["branch"], serde_json::Value::Null, "{detached}");
    assert_eq!(detached["head"]["kind"], "detached", "{detached}");
    let sha = detached["head"]["sha"].as_str().unwrap_or_default();
    assert!(sha.len() >= 7, "a short sha: {detached}");
    let clean = row("owner/clean");
    assert_eq!(clean["head"]["kind"], "branch", "{clean}");
    assert_eq!(clean["head"]["name"], clean["branch"], "{clean}");
    assert!(
        body.contains("\"head\":{\"kind\":\"detached\",\"sha\":\""),
        "{body}"
    );
    assert!(
        body.contains("\"head\":{\"kind\":\"branch\",\"name\":\""),
        "{body}"
    );
}
