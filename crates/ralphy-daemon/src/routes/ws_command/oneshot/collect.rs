//! The Query and Mutate verbs of `/ws/command`: a config CLI invocation
//! spawned and collected, answered as one reply (ADR-0036 §2).

use std::path::Path;

use super::{collect_config, not_answered, spawn_cwd};
use crate::{dispatch, protocol, session};

type Argv = Result<Vec<String>, dispatch::ArgvError>;

/// The argv of a Query verb and the reply field its output is parsed into.
fn query_argv(verb: dispatch::Verb, payload: &serde_json::Value) -> (Argv, &'static str) {
    match verb {
        dispatch::Verb::ConfigGet => (dispatch::config_argv(verb, payload), "config"),
        dispatch::Verb::BoardList => (Ok(dispatch::board_argv()), "board"),
        dispatch::Verb::IssueShow => (dispatch::issue_show_argv(payload), "issue"),
        dispatch::Verb::BranchList => (Ok(dispatch::branch_list_argv()), "branches"),
        dispatch::Verb::WorktreeList => (Ok(dispatch::worktree_list_argv()), "checkouts"),
        dispatch::Verb::ChangesList => (Ok(dispatch::changes_list_argv()), "changes"),
        dispatch::Verb::BlobRead => (dispatch::blob_read_argv(payload), "blob"),
        dispatch::Verb::SyncStatus => (Ok(dispatch::sync_status_argv()), "sync"),
        _ => (Err(dispatch::ArgvError::BadParam("verb")), "config"),
    }
}

/// The argv of a Mutate verb.
fn mutate_argv(verb: dispatch::Verb, payload: &serde_json::Value) -> Argv {
    match verb {
        dispatch::Verb::ConfigSet | dispatch::Verb::ConfigUnset => {
            dispatch::config_argv(verb, payload)
        }
        dispatch::Verb::BranchSwitch | dispatch::Verb::BranchCreate => {
            dispatch::branch_argv(verb, payload)
        }
        dispatch::Verb::WorktreeAdd => dispatch::worktree_add_argv(payload),
        dispatch::Verb::WorktreeRemove => dispatch::worktree_remove_argv(payload),
        dispatch::Verb::LabelSet => dispatch::label_argv(payload),
        dispatch::Verb::SyncFetch | dispatch::Verb::SyncPull | dispatch::Verb::SyncPush => {
            dispatch::sync_argv(verb)
        }
        dispatch::Verb::ChangesStage
        | dispatch::Verb::ChangesUnstage
        | dispatch::Verb::ChangesDiscard => dispatch::changes_paths_argv(verb, payload),
        dispatch::Verb::ChangesCommit => dispatch::changes_commit_argv(payload),
        dispatch::Verb::RunStop => dispatch::run_stop_argv(payload),
        dispatch::Verb::ProjectRemove => dispatch::project_remove_argv(payload),
        _ => Err(dispatch::ArgvError::BadParam("verb")),
    }
}

/// Resolve the `current_dir` of the verb, then spawn and collect. `Err` is the
/// reply of a checkout that did not resolve; nothing was spawned.
async fn run(
    verb: dispatch::Verb,
    cmd: &protocol::Command,
    repo_path: &Path,
    argv: Vec<String>,
    daemon_id: Option<&str>,
) -> Result<dispatch::Collected, serde_json::Value> {
    let cwd = spawn_cwd(verb, cmd, repo_path).await?;
    Ok(collect_config(argv, cwd, daemon_id.map(str::to_owned)).await)
}

pub(super) async fn query(
    verb: dispatch::Verb,
    cmd: &protocol::Command,
    repo_path: &Path,
    daemon_id: Option<&str>,
) -> serde_json::Value {
    let (argv_result, field) = query_argv(verb, &cmd.payload);
    let argv = match argv_result {
        Ok(argv) => argv,
        Err(e) => {
            tracing::warn!(error = %e, "refused a query with invalid params");
            return serde_json::json!({ "status": "error", "message": "invalid query options" });
        }
    };
    match run(verb, cmd, repo_path, argv, daemon_id).await {
        Ok(collected) => query_reply(collected, field),
        Err(reply) => reply,
    }
}

fn query_reply(collected: dispatch::Collected, field: &str) -> serde_json::Value {
    match collected {
        dispatch::Collected::Done(Some(0), bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let parsed: serde_json::Value = serde_json::from_str(text.trim())
                .unwrap_or_else(|_| serde_json::Value::String(text.trim().to_string()));
            let mut obj = serde_json::Map::new();
            obj.insert("status".to_string(), serde_json::json!("ok"));
            obj.insert(field.to_string(), parsed);
            serde_json::Value::Object(obj)
        }
        dispatch::Collected::Done(_, bytes) => serde_json::json!({
            "status": "error",
            "message": String::from_utf8_lossy(&bytes).trim(),
        }),
        dispatch::Collected::Failed(e) => {
            tracing::warn!(error = %format!("{e:#}"), "a query command failed to run");
            serde_json::json!({ "status": "error", "message": "query read failed" })
        }
        late @ (dispatch::Collected::StillRunning | dispatch::Collected::NoSlot) => {
            not_answered(&late).expect("a late answer has a reply")
        }
    }
}

pub(super) async fn mutate(
    verb: dispatch::Verb,
    cmd: &protocol::Command,
    repo_path: &Path,
    daemon_id: Option<&str>,
    slug: &str,
    sessions: &session::SessionManager,
) -> serde_json::Value {
    if let Some(refusal) = live_console_refusal(verb, cmd, slug, sessions) {
        return refusal;
    }
    let argv = match mutate_argv(verb, &cmd.payload) {
        Ok(argv) => argv,
        Err(e) => {
            tracing::warn!(error = %e, "refused a mutation with invalid params");
            return serde_json::json!({ "status": "error", "message": "invalid mutation options" });
        }
    };
    match run(verb, cmd, repo_path, argv, daemon_id).await {
        Ok(collected) => mutate_reply(verb, collected),
        Err(reply) => reply,
    }
}

/// `worktree.remove` is refused HERE (ADR-0063 §2), before any argv is composed
/// or command spawned: the session table is the daemon's alone.
fn live_console_refusal(
    verb: dispatch::Verb,
    cmd: &protocol::Command,
    slug: &str,
    sessions: &session::SessionManager,
) -> Option<serde_json::Value> {
    if verb != dispatch::Verb::WorktreeRemove {
        return None;
    }
    let name = cmd
        .payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())?;
    sessions.console_in(slug, name).then(|| {
        serde_json::json!({
            "status": "error",
            "message": format!("worktree '{name}' has a live console: close it first"),
        })
    })
}

fn mutate_reply(verb: dispatch::Verb, collected: dispatch::Collected) -> serde_json::Value {
    match collected {
        // A clean exit's output is discarded — a Mutate reply is a status, not a
        // read — with ONE exception: `worktree.add` reports its carry-over
        // warnings (`worktree.copy`/`worktree.share` entries it skipped) on
        // stdout after the add succeeded, and those are the picker's notice.
        // Relayed under `message` when there is any; the `{status:"ok"}` shape
        // otherwise.
        dispatch::Collected::Done(Some(0), bytes) => {
            let msg = String::from_utf8_lossy(&bytes);
            let msg = msg.trim();
            if msg.is_empty() || !matches!(verb, dispatch::Verb::WorktreeAdd) {
                serde_json::json!({ "status": "ok" })
            } else {
                serde_json::json!({ "status": "ok", "message": msg })
            }
        }
        dispatch::Collected::Done(_, bytes) => {
            let msg = String::from_utf8_lossy(&bytes);
            let msg = msg.trim();
            let msg = if msg.is_empty() { "refused" } else { msg };
            serde_json::json!({ "status": "error", "message": msg })
        }
        dispatch::Collected::Failed(e) => {
            tracing::warn!(error = %format!("{e:#}"), "a mutation command failed to run");
            serde_json::json!({
                "status": "error",
                "message": "mutation write failed"
            })
        }
        late @ (dispatch::Collected::StillRunning | dispatch::Collected::NoSlot) => {
            not_answered(&late).expect("a late answer has a reply")
        }
    }
}
