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

/// Read, apply, write — as ONE step. Two pages flushing at once would
/// otherwise both read the same desk and the second write would drop the
/// first one's changes; the lock is process-wide because the store is (one
/// `desk.toml` per daemon). Nothing awaits under it: `load_from`, `save_to`
/// and `move_aside` are synchronous file reads and writes. It also guards the
/// last upload number per tab.
static DESK_WRITE: tokio::sync::Mutex<TabSeqs> = tokio::sync::Mutex::const_new(TabSeqs::new());

/// How many tabs' last upload number the daemon remembers.
const TAB_SEQS: usize = 64;

/// The last `seq` taken from each of the [`TAB_SEQS`] most recent tabs
/// (ADR-0050 amendment 2026-10-04, changes, not the desk). A page resends a
/// batch with the same `seq`; when the daemon already applied it and only the
/// reply was lost, the resend is ignored instead of undoing a change another
/// device made since. In memory: a daemon restart forgets it, and a resend
/// across a restart applies again.
struct TabSeqs(std::collections::VecDeque<(String, u64)>);

impl TabSeqs {
    const fn new() -> Self {
        TabSeqs(std::collections::VecDeque::new())
    }

    /// Whether `tab` already sent a body numbered `seq` or higher.
    fn is_old(&self, tab: &str, seq: u64) -> bool {
        self.0.iter().any(|(t, last)| t == tab && seq <= *last)
    }

    /// Remember `seq` as the last one taken from `tab`, newest first.
    fn record(&mut self, tab: &str, seq: u64) {
        self.0.retain(|(t, _)| t != tab);
        self.0.push_front((tab.to_string(), seq));
        self.0.truncate(TAB_SEQS);
    }
}

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

/// The checks a desk body passes before anything is written: every rect on
/// the stage (`desk::rect_is_sane`) and every checkout one path component
/// (`checkout::lexical`). Checked on an uploaded desk version, which must
/// not store what a desk change would refuse.
fn refuse_records(
    windows: &[desk::DeskRecord],
    fences: &[desk::DeskFence],
    notes: &[desk::DeskNote],
    checkouts: &std::collections::BTreeMap<String, String>,
) -> Option<Response> {
    if let Some(bad) = windows.iter().find(|r| !desk::rect_is_sane(&r.rect)) {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("record {} has an out-of-frame rect", bad.id) }),
            ),
        )
            .into_response());
    }
    if let Some(bad) = fences.iter().find(|f| !desk::rect_is_sane(&f.rect)) {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("fence {} has an out-of-frame rect", bad.id) }),
            ),
        )
            .into_response());
    }
    if let Some(bad) = notes.iter().find(|n| !desk::rect_is_sane(&n.rect)) {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("note {} has an out-of-frame rect", bad.id) }),
            ),
        )
            .into_response());
    }
    if let Some((repo, name)) = checkouts
        .iter()
        .find(|(_, n)| checkout::lexical(n).is_none())
    {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("checkout {name} for {repo} is not a valid name") }),
            ),
        )
            .into_response());
    }
    // #411: a per-record checkout is the same kind of name as the per-repo
    // selection, gated the same way before anything is written. A note card
    // carries the same key (ADR-0064 §4, identity is `(checkout, path)`).
    let record_checkouts = windows
        .iter()
        .map(|r| (r.id.as_str(), r.checkout.as_deref()))
        .chain(notes.iter().map(|n| (n.id.as_str(), n.checkout.as_deref())));
    if let Some((id, name)) = record_checkouts
        .filter_map(|(id, c)| c.map(|n| (id, n)))
        .find(|(_, n)| checkout::lexical(n).is_none())
    {
        return Some((
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("checkout {name} on record {id} is not a valid name") }),
            ),
        )
            .into_response());
    }
    None
}

/// The `409 {"state":"restored"}` reply: the page must reload before it
/// writes. Sent for a page that loaded before a restore and for a body
/// without `changes`, which only a page from before the change list sends.
fn reload_first(error: &str) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({ "state": "restored", "error": error })),
    )
        .into_response()
}

/// The desk as a PUT answers it: the stored desk, with `rev`, plus the
/// changes it skipped.
fn put_reply(store: &desk::DeskStore, refused: &[desk::apply::Refusal]) -> Response {
    let mut body = match serde_json::to_value(store) {
        Ok(body) => body,
        Err(e) => return failed(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    };
    if let Some(map) = body.as_object_mut() {
        map.insert("refused".to_string(), serde_json::json!(refused));
    }
    Json(body).into_response()
}

/// `PUT /api/desk`: apply a list of desk changes (ADR-0050 amendment
/// 2026-10-04, changes, not the desk). The body is
/// `{ seq, generation, changes }`; `desk::apply::apply` applies the changes
/// in arrival order, and the reply is the stored desk with its `rev` and
/// `refused: [{ index, error }]` for each change it skipped.
///
/// The body is read as raw JSON because a missing `changes` means something:
/// a page from before the change list, refused with `409 {"state":"restored"}`
/// so a current page reloads. Not JSON is `400`; a bare array or any other
/// shape is `422` (#340). A page that loaded before a restore is
/// `409 restored` too. A `desk.toml` that cannot be read is refused as `409`,
/// the same reply as the GET (ADR-0070 D4). Every refusal returns BEFORE any
/// write.
///
/// A body whose `seq` is not higher than the last one this tab sent is
/// ignored and answered with the current desk ([`TabSeqs`]).
///
/// Repo keys are mapped through the registry's `former_slugs` before the
/// changes apply, so `desk.toml` converges on canonical keys whichever tab
/// writes.
///
/// A write that changes the stored desk raises `rev`, records the desk
/// history, and pushes `desk.dirty` with the writer's `tab`, so the other
/// open tabs read it again (ADR-0070 D5). A write that changes nothing writes
/// nothing and pushes nothing.
pub(crate) async fn desk_put_route(
    path: PathBuf,
    registry_path: PathBuf,
    pushes: tokio::sync::broadcast::Sender<Push>,
    tab: Option<String>,
    raw: &[u8],
) -> Response {
    let value: serde_json::Value = match serde_json::from_slice(raw) {
        Ok(value) => value,
        Err(e) => {
            return failed(
                StatusCode::BAD_REQUEST,
                format!("the desk upload is not JSON: {e}"),
            )
        }
    };
    if value
        .as_object()
        .is_some_and(|m| !m.contains_key("changes"))
    {
        return reload_first("this page was loaded by an older version of Ralphy; reload it");
    }
    let body: desk::apply::DeskBody = match serde_json::from_value(value) {
        Ok(body) => body,
        Err(e) => {
            return failed(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("the desk upload is not a list of changes: {e}"),
            )
        }
    };
    let mut seqs = DESK_WRITE.lock().await;
    let stored = match desk::load_from(&path) {
        Ok(stored) => stored,
        Err(e) => return unreadable(&e),
    };
    let aliases = former_slug_aliases(&registry_path);
    let stored = rekey::rekey_desk(stored, &aliases);
    // A page that loaded before a restore still shows the older layout
    // (ADR-0050 amendment 2026-10-04, desk history).
    if body.generation < stored.generation {
        return reload_first("the desk was restored from its history after this page read it");
    }
    if tab.as_deref().is_some_and(|t| seqs.is_old(t, body.seq)) {
        return put_reply(&stored, &[]);
    }
    let before = stored.clone();
    let applied = desk::apply::apply(stored, &body.changes, &aliases, now_ms());
    let mut store = applied.desk;
    if applied.changed {
        store.rev = before.rev + 1;
        if let Err(e) = desk::save_to(&store, &path) {
            return failed(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
        }
        let dir = history_dir(&path);
        if let Err(e) = desk::history::capture(&dir, &before, &store, now_ms()) {
            tracing::warn!(error = %format!("{e:#}"), "desk history not written");
        }
        push(&pushes, Push::Desk { tab: tab.clone() });
    }
    if let Some(t) = tab.as_deref() {
        seqs.record(t, body.seq);
    }
    put_reply(&store, &applied.refused)
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
    // A new `generation`: a page that read the unreadable desk reloads
    // instead of writing into the new one.
    let fresh = desk::DeskStore {
        rev: 1,
        generation: u64::try_from(now_ms()).unwrap_or(1),
        ..desk::DeskStore::default()
    };
    let saved = desk::move_aside(&path, &today)
        .and_then(|moved| desk::save_to(&fresh, &path).map(|()| moved));
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

/// The desk history directory, beside `desk.toml` (ADR-0050 amendment
/// 2026-10-04, desk history).
fn history_dir(desk_path: &Path) -> PathBuf {
    desk_path.with_file_name("desk-history")
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn failed(status: StatusCode, error: String) -> Response {
    (status, Json(serde_json::json!({ "error": error }))).into_response()
}

/// Query for `GET /api/desk/history`: one version, or the list.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct HistoryQuery {
    pub id: Option<i64>,
}

/// `GET /api/desk/history`: the saved versions, newest first, without their
/// desks. With `?id=`, that one version as a whole file: the download. An
/// unknown id is `404`.
pub(crate) async fn desk_history_get_route(path: PathBuf, id: Option<i64>) -> Response {
    let dir = history_dir(&path);
    match id {
        None => match desk::history::list(&dir) {
            Ok(rows) => Json(rows).into_response(),
            Err(e) => failed(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        },
        Some(id) => match desk::history::load(&dir, id) {
            Ok(version) => Json(version).into_response(),
            Err(e) if desk::history::is_not_found(&e) => {
                failed(StatusCode::NOT_FOUND, format!("no desk version {id}"))
            }
            Err(e) => failed(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        },
    }
}

/// Body of `POST /api/desk/history`: a saved version by `id`, or an uploaded
/// version file. Exactly one.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreBody {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub version: Option<desk::history::Version>,
}

/// `POST /api/desk/history`: make a saved or uploaded version the desk.
///
/// Under the desk lock: the current desk is saved as a `before-restore`
/// version first, and a failure there stops the restore, because it could
/// not be undone. `desk::history::restore` builds the new desk, the cap pins
/// every running console, `rev` rises, and `generation` becomes now, so a page that read
/// the desk before is refused on its next PUT and reloads. The result is
/// saved as a `restore` or `upload` version, and `desk.dirty` is pushed. An
/// upload passes the same checks as a PUT body. Replies `{ generation }`.
pub(crate) async fn desk_history_restore_route(
    path: PathBuf,
    registry_path: PathBuf,
    pushes: tokio::sync::broadcast::Sender<Push>,
    sessions: Arc<session::SessionManager>,
    body: RestoreBody,
) -> Response {
    use desk::history::{self, Reason};
    let dir = history_dir(&path);
    let _held = DESK_WRITE.lock().await;
    let (saved, reason) = match (body.id, body.version) {
        (Some(id), None) => match history::load(&dir, id) {
            Ok(v) => (v.desk, Reason::Restore),
            Err(e) if history::is_not_found(&e) => {
                return failed(StatusCode::NOT_FOUND, format!("no desk version {id}"));
            }
            Err(e) => return failed(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        },
        (None, Some(v)) => {
            if v.kind != history::VERSION_KIND {
                return failed(
                    StatusCode::BAD_REQUEST,
                    "the file is not a desk version".to_string(),
                );
            }
            let d = &v.desk;
            if let Some(refusal) = refuse_records(&d.windows, &d.fences, &d.notes, &d.checkouts) {
                return refusal;
            }
            (v.desk, Reason::Upload)
        }
        _ => {
            return failed(
                StatusCode::BAD_REQUEST,
                "send either `id` or `version`".to_string(),
            );
        }
    };
    let current = match desk::load_from(&path) {
        Ok(current) => current,
        Err(e) => return unreadable(&e),
    };
    let now = now_ms();
    if let Err(e) = history::append(&dir, &current, Reason::BeforeRestore, now) {
        return failed(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not save the current desk before the restore: {e:#}"),
        );
    }
    let (restored, mut pinned) = history::restore(current, saved, now);
    pinned.extend(sessions.list().into_iter().filter_map(|info| info.record));
    let generation = u64::try_from(now).unwrap_or(1);
    let store = rekey::rekey_desk(
        desk::DeskStore {
            rev: restored.rev + 1,
            generation,
            windows: desk::prune(restored.windows, &pinned),
            fences: desk::prune_fences(restored.fences),
            notes: desk::prune_notes(restored.notes),
            checkouts: restored.checkouts,
        },
        &former_slug_aliases(&registry_path),
    );
    if let Err(e) = desk::save_to(&store, &path) {
        return failed(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
    }
    if let Err(e) = history::append(&dir, &store, reason, now) {
        tracing::warn!(error = %format!("{e:#}"), "desk history not written after a restore");
    }
    push(&pushes, Push::Desk { tab: None });
    Json(serde_json::json!({ "generation": generation })).into_response()
}

/// `/api/desk`, `/api/desk/new` and `/api/desk/history`.
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
                move |Query(q): Query<DeskPutQuery>, raw: axum::body::Bytes| {
                    let (path, registry, pushes) = (path.clone(), registry.clone(), pushes.clone());
                    async move { desk_put_route(path, registry, pushes, q.tab, &raw).await }
                }
            }),
        )
        .route(
            "/api/desk/history",
            get({
                let path = s.desk_path.clone();
                move |Query(q): Query<HistoryQuery>| desk_history_get_route(path.clone(), q.id)
            })
            .post({
                let path = s.desk_path.clone();
                let registry = s.registry_path.clone();
                let pushes = s.pushes.clone();
                let sessions = s.sessions.clone();
                move |Json(body): Json<RestoreBody>| {
                    desk_history_restore_route(
                        path.clone(),
                        registry.clone(),
                        pushes.clone(),
                        sessions.clone(),
                        body,
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
