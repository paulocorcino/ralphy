//! The registry verbs on `/ws/command` and `/api/peer/command` (ADR-0036
//! amendment "the registry verbs"): `dir.list` and `project.add`. They name a
//! daemon, not a repo. The daemon that owns the disk serves them; the browser's
//! daemon relays them to a peer, and the peer never routes them again.

use std::path::{Path, PathBuf};

use super::{collect_config, not_answered};
use crate::dispatch::{self, Verb};
use crate::protocol::Command;
use crate::{dir_list, registry};

/// Serve a registry verb against THIS daemon's disk and registry. One reply
/// frame either way.
pub(crate) async fn serve_registry(
    cmd: &Command,
    verb: Verb,
    registry_path: &Path,
    daemon_id: Option<&str>,
) -> serde_json::Value {
    match verb {
        Verb::DirList => dir_list_reply(&cmd.payload, registry_path).await,
        Verb::ProjectAdd => project_add_reply(&cmd.payload, registry_path, daemon_id).await,
        _ => serde_json::json!({ "status": "error", "message": "unknown verb" }),
    }
}

async fn dir_list_reply(payload: &serde_json::Value, registry_path: &Path) -> serde_json::Value {
    let path = match payload.get("path") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => {
            return serde_json::json!({ "status": "error", "message": "invalid folder path" });
        }
    };
    let registry_path = registry_path.to_path_buf();
    let listed = tokio::task::spawn_blocking(move || {
        let store = registry::load_from(&registry_path)?;
        let home = ralphy_proc_util::home_dir();
        anyhow::Ok(dir_list::list(path.as_deref(), &store, home.as_deref()))
    })
    .await;
    match listed {
        Ok(Ok(Ok(listing))) => {
            let mut reply = serde_json::to_value(&listing).expect("a listing always serializes");
            reply["status"] = serde_json::json!("ok");
            reply
        }
        Ok(Ok(Err(refusal))) => {
            serde_json::json!({ "status": "error", "message": refusal.message() })
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %format!("{e:#}"), "failed to load the repo registry for dir.list");
            serde_json::json!({ "status": "error", "message": "repo registry unreadable" })
        }
        Err(e) => {
            tracing::warn!(error = %e, "the dir.list task did not finish");
            serde_json::json!({ "status": "error", "message": "folder listing failed" })
        }
    }
}

async fn project_add_reply(
    payload: &serde_json::Value,
    registry_path: &Path,
    daemon_id: Option<&str>,
) -> serde_json::Value {
    // The argv checks run before anything spawns: a refusal is one error frame.
    let argv = match dispatch::project_add_argv(payload) {
        Ok(argv) => argv,
        Err(e) => {
            tracing::warn!(error = %e, "refused a project.add with invalid params");
            return serde_json::json!({ "status": "error", "message": "invalid folder path" });
        }
    };
    let path = PathBuf::from(argv.last().expect("the argv ends with the path"));
    // `daemon add` takes the path as given; the store directory is a cwd that
    // always exists and names no project.
    let cwd = registry_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    match collect_config(argv, cwd, daemon_id.map(str::to_owned)).await {
        dispatch::Collected::Done(Some(0), _) => added_reply(registry_path, path).await,
        dispatch::Collected::Done(_, bytes) => serde_json::json!({
            "status": "error",
            "message": cli_message(&bytes),
        }),
        dispatch::Collected::Failed(e) => {
            tracing::warn!(error = %format!("{e:#}"), "project.add failed to run");
            serde_json::json!({ "status": "error", "message": "project add failed to run" })
        }
        late @ (dispatch::Collected::StillRunning | dispatch::Collected::NoSlot) => {
            not_answered(&late).expect("a late answer has a reply")
        }
    }
}

/// The failing CLI's own text, without the `Error: ` that `main` prints first.
fn cli_message(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    text.strip_prefix("Error: ").unwrap_or(text).to_string()
}

/// `daemon add` wrote the registry; read it back for the slug and the path it
/// registered. The entry is the one whose root holds the requested folder
/// (a subfolder registers its repo root); the deepest root wins.
async fn added_reply(registry_path: &Path, requested: PathBuf) -> serde_json::Value {
    let registry_path = registry_path.to_path_buf();
    let found = tokio::task::spawn_blocking(move || {
        let store = registry::load_from(&registry_path)?;
        let target = std::fs::canonicalize(&requested)?;
        let best = store
            .repos
            .iter()
            .filter_map(|(slug, e)| {
                let root = std::fs::canonicalize(&e.path).ok()?;
                target
                    .starts_with(&root)
                    .then(|| (root, slug.clone(), e.path.clone()))
            })
            .max_by_key(|(root, _, _)| root.components().count());
        anyhow::Ok(best.map(|(_, slug, path)| (slug, path)))
    })
    .await;
    match found {
        Ok(Ok(Some((slug, path)))) => {
            let name = registry::project_name(&slug, &path);
            serde_json::json!({ "status": "ok", "slug": slug, "name": name, "path": path })
        }
        Ok(Ok(None)) => {
            serde_json::json!({ "status": "error", "message": "the project was not found in the registry after the add" })
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %format!("{e:#}"), "could not read back the added project");
            serde_json::json!({ "status": "error", "message": "repo registry unreadable" })
        }
        Err(e) => {
            tracing::warn!(error = %e, "the project.add read-back task did not finish");
            serde_json::json!({ "status": "error", "message": "repo registry unreadable" })
        }
    }
}
