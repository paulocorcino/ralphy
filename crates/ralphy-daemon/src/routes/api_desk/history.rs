//! The desk history routes: list or download a saved version, and restore one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::{
    failed, former_slug_aliases, history_dir, now_ms, refuse_records, unreadable, DESK_WRITE,
};
use crate::routes::{push, PeerRepoCache, Push};
use crate::{desk, rekey, session};

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

/// The desk a restore puts back, and why: a saved version by id, or an
/// uploaded version that passes the checks of a PUT body.
fn restore_source(
    dir: &Path,
    body: RestoreBody,
) -> Result<(desk::DeskStore, desk::history::Reason), Box<Response>> {
    use desk::history::{self, Reason};
    match (body.id, body.version) {
        (Some(id), None) => match history::load(dir, id) {
            Ok(v) => Ok((v.desk, Reason::Restore)),
            Err(e) if history::is_not_found(&e) => Err(Box::new(failed(
                StatusCode::NOT_FOUND,
                format!("no desk version {id}"),
            ))),
            Err(e) => Err(Box::new(failed(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("{e:#}"),
            ))),
        },
        (None, Some(v)) => {
            if v.kind != history::VERSION_KIND {
                return Err(Box::new(failed(
                    StatusCode::BAD_REQUEST,
                    "the file is not a desk version".to_owned(),
                )));
            }
            let d = &v.desk;
            if let Some(refusal) = refuse_records(&d.windows, &d.fences, &d.notes, &d.checkouts) {
                return Err(Box::new(refusal));
            }
            Ok((v.desk, Reason::Upload))
        }
        _ => Err(Box::new(failed(
            StatusCode::BAD_REQUEST,
            "send either `id` or `version`".to_owned(),
        ))),
    }
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
    peers: PeerRepoCache,
    pushes: tokio::sync::broadcast::Sender<Push>,
    sessions: Arc<session::SessionManager>,
    body: RestoreBody,
) -> Response {
    use desk::history::{self, Reason};
    let dir = history_dir(&path);
    let _held = DESK_WRITE.lock().await;
    let (saved, reason) = match restore_source(&dir, body) {
        Ok(source) => source,
        Err(refusal) => return *refusal,
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
        &former_slug_aliases(&registry_path, &peers),
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
