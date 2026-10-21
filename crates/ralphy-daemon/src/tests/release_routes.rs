//! `/api/about`, `/api/release` and the refusal of an update.

use super::*;

#[tokio::test]
async fn api_about_route_reports_version_and_facts() {
    let resp = get("/api/about").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    // The build-embedded version string (git tag or Cargo fallback) — never empty.
    assert!(
        body.contains("\"version\":\"") && !body.contains("\"version\":\"\""),
        "about must carry a non-empty version; got: {body}"
    );
    assert!(
        body.contains("GPL-3.0"),
        "about must carry the license; got: {body}"
    );
    assert!(
        body.contains("Paulo Corcino"),
        "about must carry the creator; got: {body}"
    );
}

/// The workbench half of ADR-0056 is JS/HTML/CSS that no Rust gate compiles.
/// CI runs `node --test`, but that suite calls module functions; it never
/// renders this markup, and Playwright — which would — does not run there.
/// So this pin over the served assets is what reds `cargo test` if the
/// surface is deleted.
#[test]
fn the_release_badge_and_panel_are_pinned_in_the_served_assets() {
    let html = include_str!("../../assets/ui/index.html");
    let app = shell_family();
    let module = include_str!("../../assets/ui/wb-release.ts");
    let dialogs = include_str!("../../assets/ui/wb-release-dialogs.ts");
    let css = served_css();

    // The dot renders the daemon's severity; it must not be computed here.
    assert!(html.contains("class=\"rel-dot\" :class=\"$store.release.release.severity\""));
    assert!(html.contains("x-show=\"$store.release.releaseUnread\""));
    // The menu row asks the dialogs' own component to open (ADR-0073 D5).
    assert!(html.contains("$dispatch('workbench:whats-new-open')"));
    assert!(html.contains("@workbench:whats-new-open.window=\"openWhatsNew()\""));
    assert!(html.contains("x-bind=\"$store.modals.scrim('whatsNewOpen', () => closeWhatsNew())\""));
    // The whole gap, not just the newest release.
    assert!(html.contains("x-for=\"entry in $store.release.release.gap\""));
    // The upgrade is a command the operator runs, never a button that
    // replaces a binary on the daemon's host from the browser.
    assert!(html.contains("ralphy update"));

    assert!(
        app.contains("this.loadRelease()"),
        "the store `release` reads it at init"
    );
    assert!(
        app.contains("WBRelease.isSticky(this.release)"),
        "an urgent release must survive a dismissal"
    );
    // The panel's flag is in wb-release-dialogs.ts; the shortcuts ask the
    // modal stack by its path (ADR-0073 D5).
    assert!(
        app.contains("if (this.modalOpen(WBReleaseDialogs.whatsNewFlag)) return true;"),
        "the panel must join the focus trap"
    );
    assert!(
        dialogs.contains("whatsNewFlag: \"whatsNewOpen\""),
        "the flag path the shortcuts ask is the one the panel gives to scrim()"
    );

    assert!(module.contains("apiFetch('GET /api/release'"));
    assert!(
        module.contains("view.severity !== 'none'"),
        "loudness is the daemon's answer, not a browser-side derivation"
    );

    assert!(css.contains(".rel-dot.urgent"));

    // Every custom property the release block reads must be defined, or the
    // declaration is invalid at computed-value time and the property falls
    // back to its initial value. Not a hypothetical: `var(--muted)` and
    // `var(--panel)` are not in this palette, and the quiet dot — the
    // fixes-only severity, the most common one — rendered with no fill at
    // all while both `is_visible()` and a width assertion passed.
    let start = css
        .find("The release dot and the What's new panel")
        .expect("styles.css: the release block moved");
    let mut missing = Vec::new();
    for (at, _) in css[start..].match_indices("var(--") {
        let name = css[start + at + 4..]
            .split(')')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if !css.contains(&format!("{name}:")) {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "the release styles read custom properties this palette does not define: {missing:?}"
    );
}

#[tokio::test]
async fn api_release_watch_turns_the_watch_off_and_on() {
    // A mutating route with no test at all: it calls
    // `set_watch_disabled_in(dir, !enable)`, and a sign flip would turn the
    // watch OFF when the operator asks to check again, silently.
    let dir = std::env::temp_dir().join(format!("ralphy-watch-route-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    let registry = dir.join("repos.toml");

    let post = |enable: &'static str| {
        let registry = registry.clone();
        async move {
            router(
                None,
                registry,
                PathBuf::from("does-not-exist"),
                StorePaths::default(),
                Instant::now(),
                idle_shutdown(),
                auth::AuthState::localhost(),
            )
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/release/watch")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("enable={enable}")))
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };

    let resp = post("false").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8_lossy(&body).contains("\"enabled\":false"),
        "the answer states what took: {}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        release::watch_disabled_in(&dir),
        "enable=false must write the marker the poll reads"
    );

    let resp = post("true").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        !release::watch_disabled_in(&dir),
        "enable=true must remove it, not set it — the inversion is the bug this guards"
    );

    // Idempotent both ways, like the require-login toggle it is modelled on.
    assert_eq!(post("true").await.status(), StatusCode::OK);
    assert!(!release::watch_disabled_in(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn api_release_answers_from_the_cache_without_reaching_the_network() {
    // The route reads only what the background watch cached: a page load
    // must never wait on github.com, nor trigger a request of its own.
    let resp = get("/api/release").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let view: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(
        view["current"].as_str().is_some_and(|v| !v.is_empty()),
        "the view must name the running build; got: {view}"
    );
    assert_eq!(view["channel"], "rc");
    assert!(
        ["behind", "level", "ahead", "unknown"]
            .contains(&view["standing"].as_str().unwrap_or_default()),
        "standing is a closed set; got: {view}"
    );
    assert!(view["gap"].is_array());
    // The watch is spawned by `serve`, never by `router`: building a router
    // must not reach the network, nor drop a cache beside the test.
    assert!(
        !std::path::Path::new("releases.json").exists(),
        "a router built in a test must not have written a release cache"
    );
    // This tree's binary is built from a working copy, so it is ahead of its
    // tag and is never offered an update — whatever happens to be cached.
    assert_eq!(view["severity"], "none");
    assert_eq!(
        view["can_update"], false,
        "a build ahead of its tag has nothing to take; got: {view}"
    );
}

/// POST `/api/release/update` on the shared test router, with `auth` as the
/// `Authorization` header when given.
async fn post_update(auth: Option<&str>) -> Response {
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/release/update")
        .header("content-type", "application/x-www-form-urlencoded");
    if let Some(value) = auth {
        req = req.header("authorization", value);
    }
    router(
        None,
        PathBuf::from("does-not-exist"),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        Instant::now(),
        idle_shutdown(),
        auth::AuthState::localhost(),
    )
    .oneshot(req.body(Body::empty()).unwrap())
    .await
    .unwrap()
}

#[tokio::test]
async fn the_update_is_refused_to_a_machine_client() {
    // The update ends every console. A bearer token is how a script
    // authenticates, so it must not be enough to start one.
    let resp = post_update(Some("Bearer anything")).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_update_is_refused_to_a_build_with_nothing_to_take() {
    // This test binary is ahead of its tag: the route must refuse before it
    // asks for a code or starts a child.
    let resp = post_update(None).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8_lossy(&body).contains("ralphy update"),
        "the refusal names the way that still works: {}",
        String::from_utf8_lossy(&body)
    );
}
