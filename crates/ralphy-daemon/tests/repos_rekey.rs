//! The registry's self-heal on a gained remote (ADR-0036 amendment
//! 2026-09-16): `GET /api/repos` notices a `path-<hash>` entry whose repo now
//! has an `origin`, hands it to `ralphy daemon add` — the test child here,
//! standing in for the CLI's registrar — re-reads the registry, and answers
//! with the migrated key on that SAME request. And it does so once: a second
//! read spawns nothing for a pair already claimed.
//!
//! SOLE env-setter in its file: `RALPHY_EXE_OVERRIDE`/`RALPHY_TEST_*` are
//! process-global, which is also why the two phases are one test — the second
//! phase must run with the re-key knob UNSET, and a sibling test could not be
//! ordered against that.

use std::path::Path;
use std::time::Instant;

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use ralphy_daemon::{registry, router};
use tower::ServiceExt;

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git (CI and the build machine have git)");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn app(registry_path: &Path) -> axum::Router {
    router(
        None,
        registry_path.to_path_buf(),
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        tokio::sync::watch::channel(false).1,
        ralphy_daemon::auth::AuthState::localhost(),
    )
}

async fn repos(app: &axum::Router) -> String {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/repos")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
}

#[tokio::test]
async fn api_repos_rekeys_a_path_slug_once_the_repo_has_a_remote() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init"]);
    git(
        &repo,
        &["remote", "add", "origin", "https://github.com/o/r.git"],
    );
    let registry_path = dir.path().join("repos.toml");
    let argv_file = dir.path().join("argv.log");

    let seed = || {
        let mut store = registry::RegistryStore::default();
        store.upsert("path-abc", &repo.to_string_lossy().replace('\\', "/"));
        registry::save_to(&store, &registry_path).unwrap();
    };

    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "0");
    std::env::set_var("RALPHY_TEST_ARGV_FILE", &argv_file);

    // PHASE 1 — the registrar re-keys: the FIRST read already answers `o/r`.
    seed();
    std::env::set_var(
        "RALPHY_TEST_REGISTRY_REKEY",
        format!("{}|path-abc|o/r", registry_path.display()),
    );
    let body = repos(&app(&registry_path)).await;
    assert!(
        body.contains(r#""slug":"o/r""#) && !body.contains("path-abc"),
        "the same request that noticed the remote serves the new key: {body}"
    );
    let log = std::fs::read_to_string(&argv_file).unwrap();
    let repo_arg = repo.to_string_lossy().replace('\\', "/");
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        vec![format!("daemon add {repo_arg}").as_str()],
        "one registrar spawn, `daemon add <path>`, never --init: {log}"
    );

    // PHASE 2 — a registrar that changes nothing (the remote's URL yields no
    // forge slug) is spawned ONCE per router lifetime, not per page load.
    std::env::remove_var("RALPHY_TEST_REGISTRY_REKEY");
    std::fs::remove_file(&argv_file).unwrap();
    seed();
    let app = app(&registry_path);
    let first = repos(&app).await;
    let second = repos(&app).await;
    assert!(
        first.contains(r#""slug":"path-abc""#) && second.contains(r#""slug":"path-abc""#),
        "an unhealed key is still served: {second}"
    );
    let log = std::fs::read_to_string(&argv_file).unwrap();
    assert_eq!(
        log.lines().count(),
        1,
        "the second read must not respawn the registrar: {log}"
    );
}
