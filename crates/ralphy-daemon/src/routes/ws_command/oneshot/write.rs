//! The Write verbs of `/ws/command` that answer in the daemon: the file
//! mutations of the editor and the clipboard drop (ADR-0036 §2, ADR-0055).

use std::path::Path;

use super::{encoding_param, note_write};
use crate::routes::blocking_read;
use crate::{clipboard, dispatch, fswrite, protocol};

/// Answer one Write verb. A note write is the ONE Write a `checkout` is
/// honoured on (ADR-0064 §5): it is answered above the refusal below, because
/// it does not confine a client path against the PRIMARY root the way its Write
/// siblings do.
pub(super) async fn write(
    verb: dispatch::Verb,
    cmd: &protocol::Command,
    repo_path: &Path,
) -> serde_json::Value {
    if verb == dispatch::Verb::NoteWrite {
        return note_write(cmd, repo_path).await;
    }
    // A write under a `checkout` is refused BEFORE any arm: the `.ralphy`
    // denylist (`fswrite::PROTECTED_DIRS`) could never let a prefixed target
    // through, and silently dropping the key would land a Save from a tab
    // showing worktree bytes on the PRIMARY's file at the same rel (ADR-0063 §2;
    // lifted in a later slice).
    if cmd.payload.get("checkout").is_some_and(|v| !v.is_null()) {
        return serde_json::json!({
            "status": "error",
            "reason": "refused",
            "message": "Writing inside a worktree is not available yet.",
        });
    }
    if verb == dispatch::Verb::ImageWrite {
        return image_write(cmd, repo_path).await;
    }
    file_verb(verb, cmd, repo_path)
}

/// A clipboard drop (ADR-0055) answers on its own: unlike its Write siblings its
/// success reply carries the PATH the daemon chose, and it reads no `path` from
/// the client at all. Validation lives here — decode, cap, sniff — so
/// `clipboard::write_image` stays a dumb confined writer of bytes already known
/// to be an image.
async fn image_write(cmd: &protocol::Command, repo_path: &Path) -> serde_json::Value {
    let root = repo_path.to_path_buf();
    let outcome = match clipboard::decode_image(cmd.payload.get("base64").and_then(|v| v.as_str()))
    {
        Err(reason) => Some(Err(reason)),
        Ok((kind, bytes)) => blocking_read(move || clipboard::write_image(&root, kind, &bytes))
            .await
            .map(|r| r.map_err(clipboard::write_reason)),
    };
    match outcome {
        Some(Ok(path)) => serde_json::json!({ "status": "ok", "path": path }),
        Some(Err(reason)) => serde_json::json!({ "status": "error", "reason": reason }),
        None => serde_json::json!({ "status": "error", "reason": "unavailable" }),
    }
}

fn file_verb(verb: dispatch::Verb, cmd: &protocol::Command, repo_path: &Path) -> serde_json::Value {
    let rel = cmd
        .payload
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let to = cmd.payload.get("to").and_then(|v| v.as_str()).unwrap_or("");
    match verb {
        dispatch::Verb::FileWrite => file_write(cmd, repo_path, rel),
        dispatch::Verb::FileCreate => {
            let dir = cmd
                .payload
                .get("dir")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            write_reply(fswrite::create(repo_path, rel, dir))
        }
        dispatch::Verb::FileRename => write_reply(fswrite::rename(repo_path, rel, to)),
        dispatch::Verb::FileCopy => write_reply(fswrite::copy(repo_path, rel, to)),
        dispatch::Verb::FileDelete => write_reply(fswrite::delete(repo_path, rel)),
        dispatch::Verb::PlanDiscard => write_reply(fswrite::discard_plan(repo_path)),
        _ => write_reply(Err(fswrite::WriteError::Io)),
    }
}

fn file_write(cmd: &protocol::Command, repo_path: &Path, rel: &str) -> serde_json::Value {
    let content = cmd
        .payload
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // Absent `encoding` is UTF-8 without a BOM: every client that predates the
    // encoding amendment keeps its bytes.
    let encoding = match encoding_param(cmd) {
        Ok(hint) => hint.unwrap_or(encoding_rs::UTF_8),
        Err(refusal) => return refusal,
    };
    let bom = cmd
        .payload
        .get("bom")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    write_reply(fswrite::write_encoded(
        repo_path, rel, content, encoding, bom,
    ))
}

fn write_reply(result: Result<(), fswrite::WriteError>) -> serde_json::Value {
    match result {
        Ok(()) => serde_json::json!({ "status": "ok" }),
        Err(e @ fswrite::WriteError::Unencodable { char_index }) => serde_json::json!({
            "status": "error",
            "reason": e.reason(),
            "char_index": char_index,
        }),
        Err(e) => serde_json::json!({ "status": "error", "reason": e.reason() }),
    }
}
