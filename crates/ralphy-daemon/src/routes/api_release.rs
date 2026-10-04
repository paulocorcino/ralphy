//! `/api/release*`: where this build stands against what has been published
//! (ADR-0056), and the routes of the release area.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::Form;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;

use super::{
    release_update_route, release_watch_route, ReleaseWatchForm, RouterShared, UpdateForm,
};
use crate::release;

/// How often the daemon asks what has been published. Four reads a day notices a
/// release cut this morning and cannot contribute to exhausting the
/// unauthenticated rate limit (ADR-0056 §6).
pub(crate) const RELEASE_POLL_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// One pass of the release watch. Blocking (it is `ureq`), silent on failure by
/// construction, and a no-op when the operator turned the watch off.
pub(crate) fn poll_releases(store: &Path) {
    if release::watch_disabled_in(store) {
        return;
    }
    let cache = release::cache_path_in(store);
    ralphy_release::fetch::refresh_if_stale(&ralphy_release::RefreshOpts::new(&cache));
}

/// `GET /api/release`: where this build stands against what has been published,
/// and the whole gap between the two.
///
/// Reads the cache only — the fetch is the background watch's job, so a page
/// load never waits on the network and never triggers a request of its own.
/// A store the daemon could not resolve answers the same shape with nothing in
/// it, because "we do not know" is a normal state, not an error.
pub(crate) async fn release_route(store: Option<PathBuf>) -> Response {
    let (releases, disabled) = match store.as_deref() {
        Some(dir) => (
            ralphy_release::fetch::load(&release::cache_path_in(dir)),
            release::watch_disabled_in(dir),
        ),
        None => (Vec::new(), false),
    };
    let view = release::view(
        env!("RALPHY_VERSION"),
        &releases,
        ralphy_release::Channel::Rc,
        disabled,
    );
    let can_update = store.is_some() && super::updatable(&view.standing, release::under_systemd());
    Json(ReleaseReply { view, can_update }).into_response()
}

/// The view, plus whether the workbench may offer the update (ADR-0056 §11).
/// A wrapper, so the public `ReleaseView` keeps its shape.
#[derive(serde::Serialize)]
struct ReleaseReply {
    #[serde(flatten)]
    view: release::ReleaseView,
    can_update: bool,
}

/// `/api/release*`: what has been published, the watch switch, and the update.
pub(crate) fn release_routes(s: &RouterShared) -> Router {
    Router::new()
        .route(
            "/api/release",
            get({
                let store = s.release_store.clone();
                move || release_route(store.clone())
            }),
        )
        .route(
            "/api/release/watch",
            post({
                let store = s.release_store.clone();
                move |form: Form<ReleaseWatchForm>| release_watch_route(store.clone(), form)
            }),
        )
        .route(
            "/api/release/update",
            post({
                let auth = s.auth.clone();
                let store = s.release_store.clone();
                move |headers: axum::http::HeaderMap, form: Form<UpdateForm>| {
                    release_update_route(auth.clone(), store.clone(), headers, form)
                }
            }),
        )
}
