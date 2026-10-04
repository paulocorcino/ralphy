//! The desk routes: read it, write it, and start a new one when the saved
//! desk cannot be read.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;

use super::RouterShared;
use super::{push, Push};
use crate::{checkout, desk, registry, rekey, session};

/// Query for `PUT /api/desk`: the writing tab's id, echoed in the
/// `desk.dirty` push so that tab does not read its own write again.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct DeskPutQuery {
    pub tab: Option<String>,
}

/// Read, fold, write — as ONE step. Two pages flushing at once would
/// otherwise both read the same desk and the second write would drop the
/// first fold; the lock is process-wide because the store is (one
/// `desk.toml` per daemon). Nothing awaits under it: `load_from`, `save_to`
/// and `move_aside` are synchronous file reads and writes.
static DESK_WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The reply for a `desk.toml` that exists but cannot be loaded (ADR-0070 D4).
/// A layout that cannot be parsed is `409 unreadable`, and the shell offers to
/// start a new desk. A file that cannot be read is `503 unavailable`: the
/// file may be fine, so nothing offers to replace it. The shell keys on
/// `state`, not on the status.
fn unreadable(e: &anyhow::Error) -> Response {
    let (status, state) = if desk::is_parse_error(e) {
        (StatusCode::CONFLICT, "unreadable")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "unavailable")
    };
    (
        status,
        Json(serde_json::json!({ "state": state, "error": format!("{e:#}") })),
    )
        .into_response()
}

/// `GET /api/desk`: the saved desk — windows and fences together, each in layout
/// order (ADR-0050, ADR-0051 §10), plus `checkouts` (the selected worktree per
/// repo ref, ADR-0063 §4) when any is set. An absent `desk.toml` answers
/// `200 {"windows":[],"fences":[]}`. One that cannot be read or parsed answers
/// `409 {"state":"unreadable","error":…}`, never an empty desk (ADR-0070 D4).
///
/// Served under the registry's CANONICAL keys: a record saved under a slug the
/// registry has since re-keyed (`former_slugs`) is rewritten on the way out,
/// so a migrated project's consoles come back to it (ADR-0036 amendment
/// 2026-09-16). The registry is read fresh, like `/api/repos`.
pub(crate) async fn desk_get_route(path: PathBuf, registry_path: PathBuf) -> Response {
    let aliases = former_slug_aliases(&registry_path);
    match desk::load_from(&path) {
        Ok(store) => Json(rekey::rekey_desk(store, &aliases)).into_response(),
        Err(e) => unreadable(&e),
    }
}

/// The registry's former-slug → canonical-key map, or empty when the registry
/// is unreadable (warned; the desk is still served — a lost alias costs a
/// cascaded stage, never the layout).
pub(crate) fn former_slug_aliases(
    registry_path: &Path,
) -> std::collections::BTreeMap<String, String> {
    match registry::load_from(registry_path) {
        Ok(store) => store.former_slug_map(),
        Err(e) => {
            tracing::warn!(error = %e, "repo registry unreadable; desk served without slug aliases");
            Default::default()
        }
    }
}

/// `PUT /api/desk`: replace the desk wholesale, each record type pruned to its
/// own cap ([`desk::DESK_MAX`], [`desk::FENCE_MAX`]) newest by `ts`, answering
/// `200` with the pruned store — the client needs the daemon's post-prune truth
/// in one round trip (last-write-wins, no ETag).
///
/// A body that is not a `{ windows, fences, checkouts? }` object — including
/// the pre-#340 bare array — is rejected by the `Json` extractor as `422` and
/// never reaches here, so `desk.toml` is untouched; a rect that is out of frame
/// — non-finite, or an origin off the stage's pinned 0,0 — and a checkout
/// value that is not one path component (`checkout::lexical`) are rejected
/// here as `400`. A `desk.toml` that cannot be read is refused as `409`, the
/// same reply as the GET (ADR-0070 D4). Every rejection returns BEFORE any
/// write, so a refused upload leaves `desk.toml` byte-identical on every path.
/// A well-shaped checkout name is stored unvalidated: whether the worktree
/// still exists is the verb's call (`unknown checkout`), not a spawn per desk
/// write.
///
/// Non-overlap between fences is deliberately NOT validated: refusing a whole
/// desk upload would cost the operator their layout and the daemon has no repair
/// path, so that invariant belongs to the client (ADR-0051 §6).
///
/// Stored under the registry's canonical keys: an upload from a tab that read
/// the desk before a re-key still names the former slug, and is normalized
/// through `former_slugs` before anything is written — so `desk.toml`
/// converges on the first save after a migration, whichever tab saves.
///
/// A write that changes the stored desk pushes `desk.dirty` with the writer's
/// `tab`, so the other open tabs read it again (ADR-0070 D5). A write that
/// changes nothing pushes nothing.
pub(crate) async fn desk_put_route(
    path: PathBuf,
    registry_path: PathBuf,
    pushes: tokio::sync::broadcast::Sender<Push>,
    sessions: Arc<session::SessionManager>,
    tab: Option<String>,
    up: desk::DeskUpload,
) -> Response {
    if let Some(bad) = up.windows.iter().find(|r| !desk::rect_is_sane(&r.rect)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("record {} has an out-of-frame rect", bad.id) }),
            ),
        )
            .into_response();
    }
    if let Some(bad) = up.fences.iter().find(|f| !desk::rect_is_sane(&f.rect)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("fence {} has an out-of-frame rect", bad.id) }),
            ),
        )
            .into_response();
    }
    if let Some(bad) = up.notes.iter().find(|n| !desk::rect_is_sane(&n.rect)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("note {} has an out-of-frame rect", bad.id) }),
            ),
        )
            .into_response();
    }
    if let Some((repo, name)) = up
        .checkouts
        .iter()
        .find(|(_, n)| checkout::lexical(n).is_none())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("checkout {name} for {repo} is not a valid name") }),
            ),
        )
            .into_response();
    }
    // #411: a per-record checkout is the same kind of name as the per-repo
    // selection, gated the same way before anything is written. A note card
    // carries the same key (ADR-0064 §4, identity is `(checkout, path)`).
    let record_checkouts = up
        .windows
        .iter()
        .map(|r| (r.id.as_str(), r.checkout.as_deref()))
        .chain(
            up.notes
                .iter()
                .map(|n| (n.id.as_str(), n.checkout.as_deref())),
        );
    if let Some((id, name)) = record_checkouts
        .filter_map(|(id, c)| c.map(|n| (id, n)))
        .find(|(_, n)| checkout::lexical(n).is_none())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("checkout {name} on record {id} is not a valid name") }),
            ),
        )
            .into_response();
    }
    let _held = DESK_WRITE.lock().await;
    let stored = match desk::load_from(&path) {
        Ok(stored) => stored,
        Err(e) => return unreadable(&e),
    };
    let before = stored.clone();
    let merged = desk::merge(stored, up);
    // The records a session of this daemon serves are never cut by the cap.
    let live: std::collections::HashSet<String> = sessions
        .list()
        .into_iter()
        .filter_map(|info| info.record)
        .collect();
    let store = rekey::rekey_desk(
        desk::DeskStore {
            windows: desk::prune(merged.windows, &live),
            fences: desk::prune_fences(merged.fences),
            notes: desk::prune_notes(merged.notes),
            checkouts: merged.checkouts,
        },
        &former_slug_aliases(&registry_path),
    );
    match desk::save_to(&store, &path) {
        Ok(()) => {
            if before != store {
                push(&pushes, Push::Desk { tab });
            }
            Json(store).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

/// `POST /api/desk/new`: start a new desk when the saved one cannot be parsed.
/// The old file is renamed to `desk.toml.unreadable-<date>` first, so nothing
/// is deleted (ADR-0070 D4). A desk that reads fine, or does not exist, is not
/// replaced: `409 {"state":"readable"}`. A file that cannot be read is not
/// replaced either: `503 {"state":"unavailable"}`.
pub(crate) async fn desk_new_route(
    path: PathBuf,
    pushes: tokio::sync::broadcast::Sender<Push>,
) -> Response {
    let _held = DESK_WRITE.lock().await;
    match desk::load_from(&path) {
        Ok(_) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "state": "readable" })),
            )
                .into_response();
        }
        Err(e) if !desk::is_parse_error(&e) => return unreadable(&e),
        Err(_) => {}
    }
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let saved = desk::move_aside(&path, &today)
        .and_then(|moved| desk::save_to(&desk::DeskStore::default(), &path).map(|()| moved));
    match saved {
        Ok(moved) => {
            push(&pushes, Push::Desk { tab: None });
            let name = moved
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Json(serde_json::json!({ "moved_to": name })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e:#}") })),
        )
            .into_response(),
    }
}

/// `/api/desk` and `/api/desk/new`.
pub(crate) fn desk_routes(s: &RouterShared) -> Router {
    Router::new()
        .route(
            "/api/desk",
            get({
                let path = s.desk_path.clone();
                let registry = s.registry_path.clone();
                move || desk_get_route(path.clone(), registry.clone())
            })
            .put({
                let path = s.desk_path.clone();
                let registry = s.registry_path.clone();
                let pushes = s.pushes.clone();
                let sessions = s.sessions.clone();
                move |Query(q): Query<DeskPutQuery>, Json(up): Json<desk::DeskUpload>| {
                    desk_put_route(
                        path.clone(),
                        registry.clone(),
                        pushes.clone(),
                        sessions.clone(),
                        q.tab,
                        up,
                    )
                }
            }),
        )
        .route(
            "/api/desk/new",
            post({
                let path = s.desk_path.clone();
                let pushes = s.pushes.clone();
                move || desk_new_route(path.clone(), pushes.clone())
            }),
        )
}
