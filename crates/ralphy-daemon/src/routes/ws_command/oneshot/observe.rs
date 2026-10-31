//! The Observe verbs: filesystem reads answered off the runtime, each
//! in the operator's coordinates (ADR-0036 §2, ADR-0063 §2).

use std::path::{Path, PathBuf};

use super::{checkout_of, encoding_param};
use crate::routes::blocking_read;
use crate::{checkout, confine, dispatch, note, protocol, textcodec, tree};

fn unavailable() -> serde_json::Value {
    serde_json::json!({ "status": "error", "reason": "unavailable" })
}

/// A string key of the payload, or the empty string when it is absent.
fn payload_str<'a>(cmd: &'a protocol::Command, key: &str) -> &'a str {
    cmd.payload.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

/// Answer one Observe verb. The optional `checkout` argument is read HERE, the
/// one place every Observe verb takes its `path`: it becomes a rel PREFIX under
/// the same registered root (`checkout::from_payload`), so confinement never
/// learns a second root. `runs.list` ignores it (runs are primary-tree state).
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "the effect class sends only Observe verbs here and `Verb` has over 60 variants"
)]
pub(super) async fn observe(
    verb: dispatch::Verb,
    cmd: &protocol::Command,
    repo_path: &Path,
) -> serde_json::Value {
    let checkout = match checkout_of(cmd, repo_path).await {
        Ok(c) => c,
        Err(reply) => return reply,
    };
    let asked = payload_str(cmd, "path");
    let rel = checkout
        .as_ref()
        .map_or_else(|| asked.to_string(), |c| c.prefix(asked));
    match verb {
        dispatch::Verb::TreeList => tree_list(repo_path, checkout.as_ref(), asked).await,
        dispatch::Verb::TreeFind | dispatch::Verb::TreeGrep => {
            tree_search(verb, cmd, repo_path, checkout.as_ref()).await
        }
        dispatch::Verb::FileRead => file_read(cmd, repo_path, &rel).await,
        dispatch::Verb::FileStat => file_stat(repo_path, checkout.as_ref(), asked).await,
        dispatch::Verb::TreeSize => tree_size(repo_path, checkout.as_ref(), asked).await,
        dispatch::Verb::ImageRead => image_read(repo_path, &rel).await,
        dispatch::Verb::NoteRead => note_read(repo_path, &rel).await,
        dispatch::Verb::RunsList => runs_list(repo_path),
        _ => serde_json::json!({ "status": "error", "reason": "refused" }),
    }
}

/// A checkout is listed from ITS root, confined against the registered root
/// first as the searches do: the ignore rules are anchored there, and the
/// primary ignores `.ralphy/`, which would mark the whole worktree ignored.
async fn tree_list(
    repo_path: &Path,
    checkout: Option<&checkout::Checkout>,
    asked: &str,
) -> serde_json::Value {
    let repo_root = repo_path.to_path_buf();
    let walk_rel = checkout.map(|c| c.prefix(""));
    // `Checkout::prefix` trims the slashes; the checkout-rooted rel keeps doing so.
    let path = if walk_rel.is_some() {
        asked.trim_matches('/').to_string()
    } else {
        asked.to_string()
    };
    let listed = blocking_read(move || {
        let root = match walk_rel {
            None => repo_root,
            Some(rel) => confine::confine(&repo_root, &rel)?,
        };
        tree::list(&root, &path)
    })
    .await;
    match listed {
        Some(Ok(entries)) => serde_json::json!({ "status": "ok", "entries": entries }),
        Some(Err(_)) => serde_json::json!({ "status": "error", "reason": "not found" }),
        None => unavailable(),
    }
}

/// The two searches (ADR-0036 amendment 2026-09-15) take `query`, not `path`:
/// they always walk from the root — the worktree's root under a `checkout`, so
/// hits come back relative to it (`search::rel_of` strips the walk root). That
/// walk root is resolved through `confine` against the REGISTERED root first: a
/// worktree dir that is a symlink out of the repo must be refused here exactly
/// as `tree.list` refuses it, never handed to the walker as a root that would
/// confine against itself. Their budget is the wire default; nothing upstream
/// caps an Observe read, so the walker stops itself and says `truncated`.
async fn tree_search(
    verb: dispatch::Verb,
    cmd: &protocol::Command,
    repo_path: &Path,
    checkout: Option<&checkout::Checkout>,
) -> serde_json::Value {
    let query = payload_str(cmd, "query").to_string();
    let repo_root = repo_path.to_path_buf();
    let settings_root = repo_path.to_path_buf();
    let walk_rel = checkout.map(|c| c.prefix(""));
    let budget = tree::SearchBudget::default();
    let find = verb == dispatch::Verb::TreeFind;
    let searched = blocking_read(move || {
        let root = match walk_rel {
            None => repo_root,
            Some(rel) => confine::confine(&repo_root, &rel)?,
        };
        if find {
            tree::find(&root, &query, &budget).map(|r| serde_json::json!(r))
        } else {
            // The primary's settings, not the checkout's: `.ralphy` is run state
            // a worktree does not carry.
            let fallback = textcodec::fallback_for(&settings_root);
            tree::grep_with(&root, &query, &budget, fallback).map(|r| serde_json::json!(r))
        }
    })
    .await;
    match searched {
        Some(Ok(mut reply)) => {
            reply["status"] = serde_json::json!("ok");
            reply
        }
        Some(Err(_)) => serde_json::json!({ "status": "error", "reason": "not found" }),
        None => unavailable(),
    }
}

/// Where the properties verbs read from: the checkout's own root when there is
/// one, and the path asked for relative to it. The ignore rules anchor at that
/// root, as for `tree.list`: read from the primary root, every file of a
/// worktree would be ignored, because the primary ignores `.ralphy/`.
fn properties_target(
    checkout: Option<&checkout::Checkout>,
    asked: &str,
) -> (Option<String>, String) {
    match checkout {
        Some(c) => (Some(c.prefix("")), asked.trim_matches('/').to_string()),
        None => (None, asked.to_string()),
    }
}

/// The checkout root, confined against the registered root as the searches do.
fn properties_root(
    repo_root: PathBuf,
    walk_rel: Option<String>,
) -> Result<PathBuf, confine::ConfineError> {
    match walk_rel {
        None => Ok(repo_root),
        Some(rel) => confine::confine(&repo_root, &rel),
    }
}

/// `file.stat`: the facts of one path, answered at once.
async fn file_stat(
    repo_path: &Path,
    checkout: Option<&checkout::Checkout>,
    asked: &str,
) -> serde_json::Value {
    let (walk_rel, path) = properties_target(checkout, asked);
    let repo_root = repo_path.to_path_buf();
    let settings_root = repo_path.to_path_buf();
    let read = blocking_read(move || {
        // A path outside the root answers like a missing one: no reply says which.
        let root = properties_root(repo_root, walk_rel).map_err(|error| match error {
            confine::ConfineError::Escape | confine::ConfineError::NotFound => {
                tree::ReadError::NotFound
            }
        })?;
        // The primary's settings, not the checkout's, as for the searches.
        tree::stat(&root, &path, textcodec::fallback_for(&settings_root))
    })
    .await;
    match read {
        Some(Ok(stat)) => ok_reply(&stat),
        Some(Err(e)) => serde_json::json!({ "status": "error", "reason": e.reason() }),
        None => unavailable(),
    }
}

/// `tree.size`: the size of one folder. The walk stops itself at its budget and
/// says `truncated`; a second walk while one runs answers `busy`.
async fn tree_size(
    repo_path: &Path,
    checkout: Option<&checkout::Checkout>,
    asked: &str,
) -> serde_json::Value {
    let (walk_rel, path) = properties_target(checkout, asked);
    let repo_root = repo_path.to_path_buf();
    let sized = blocking_read(move || {
        // A path outside the root answers like a missing one: no reply says which.
        let root = properties_root(repo_root, walk_rel).map_err(|error| match error {
            confine::ConfineError::Escape | confine::ConfineError::NotFound => {
                tree::SizeError::NotFound
            }
        })?;
        tree::size(&root, &path, tree::SizeBudget::default(), &tree::size::SLOT)
    })
    .await;
    match sized {
        Some(Ok(size)) => ok_reply(&size),
        Some(Err(e)) => serde_json::json!({ "status": "error", "reason": e.reason() }),
        None => unavailable(),
    }
}

/// A reply of `status: "ok"` that carries the fields of `value`.
fn ok_reply(value: &impl serde::Serialize) -> serde_json::Value {
    let mut reply = serde_json::json!(value);
    reply["status"] = serde_json::json!("ok");
    reply
}

async fn file_read(cmd: &protocol::Command, repo_path: &Path, rel: &str) -> serde_json::Value {
    let hint = match encoding_param(cmd) {
        Ok(hint) => hint,
        Err(refusal) => return refusal,
    };
    let (root, path) = (repo_path.to_path_buf(), rel.to_string());
    let read = blocking_read(move || {
        let fallback = textcodec::fallback_for(&root);
        tree::read_with(&root, &path, hint, fallback)
    })
    .await;
    match read {
        Some(Ok(decoded)) => serde_json::json!({
            "status": "ok",
            "content": decoded.text,
            "encoding": decoded.encoding.name(),
            "bom": decoded.bom,
        }),
        Some(Err(e)) => serde_json::json!({ "status": "error", "reason": e.reason() }),
        None => unavailable(),
    }
}

async fn image_read(repo_path: &Path, rel: &str) -> serde_json::Value {
    let (root, path) = (repo_path.to_path_buf(), rel.to_string());
    match blocking_read(move || tree::read_image(&root, &path)).await {
        Some(Ok(image)) => serde_json::json!({
            "status": "ok",
            "mediaType": image.media_type,
            "base64": data_encoding::BASE64.encode(&image.bytes),
        }),
        Some(Err(e)) => serde_json::json!({ "status": "error", "reason": e.reason() }),
        None => unavailable(),
    }
}

async fn note_read(repo_path: &Path, rel: &str) -> serde_json::Value {
    let (root, path) = (repo_path.to_path_buf(), rel.to_string());
    // The mtime rides along with the text, on the one trip the card already
    // makes: the footer says when the note was last saved, and after a reload
    // the card's own memory of that is gone (ADR-0064 §7 as amended).
    match blocking_read(move || {
        note::read(&root, &path).map(|markdown| (markdown, note::modified(&root, &path)))
    })
    .await
    {
        Some(Ok((markdown, modified))) => {
            serde_json::json!({ "status": "ok", "markdown": markdown, "modified": modified })
        }
        Some(Err(e)) => serde_json::json!({ "status": "error", "reason": e.reason() }),
        None => unavailable(),
    }
}

fn runs_list(repo_path: &Path) -> serde_json::Value {
    let listing = ralphy_run_snapshot::list_runs(repo_path, ralphy_proc_util::pid_is_alive);
    serde_json::json!({
        "status": "ok",
        "runs": listing.live,
        "unreadable": listing.unreadable,
    })
}
