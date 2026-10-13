//! The non-streaming verbs of `/ws/command`: spawn-and-collect and the
//! Observe family, answered by effect class (ADR-0036 §2, ADR-0063 §2).

use std::path::{Path, PathBuf};

use anyhow::Result;

use encoding_rs::Encoding;

use crate::routes::blocking_read;
use crate::{checkout, confine, dispatch, note, protocol, session, textcodec};

mod collect;
mod observe;
mod write;

/// The optional `encoding` of a `file.read`/`file.write` payload (ADR-0036
/// amendment 2026-09-22): a WHATWG label, or absent. A label the decoder
/// cannot name is the ready-made refusal `unknown encoding` — never a silent
/// fall-through to detection, which would answer with bytes the client did not
/// ask for.
fn encoding_param(cmd: &protocol::Command) -> Result<Option<&'static Encoding>, serde_json::Value> {
    match cmd.payload.get("encoding").and_then(|v| v.as_str()) {
        None => Ok(None),
        Some(name) => textcodec::label(name)
            .map(Some)
            .ok_or_else(|| serde_json::json!({ "status": "error", "reason": "unknown encoding" })),
    }
}

/// The reply when a Query or Mutate command passed the reply deadline. The
/// command is not stopped.
pub(crate) const STILL_RUNNING: &str =
    "The command is still running after 60 seconds. Ralphy did not stop it, and its answer will not come here.";

/// The reply when no command slot was free within the reply deadline.
pub(crate) const NO_SLOT: &str =
    "Ralphy is busy: 8 commands are still running, and this one did not start. Try again later.";

/// The reply for a Query or Mutate that gave no answer in time.
pub(crate) fn not_answered(collected: &dispatch::Collected) -> Option<serde_json::Value> {
    let message = match collected {
        dispatch::Collected::StillRunning => STILL_RUNNING,
        dispatch::Collected::NoSlot => NO_SLOT,
        _ => return None,
    };
    Some(serde_json::json!({ "status": "error", "message": message }))
}

/// Spawn-and-COLLECT a config CLI invocation (`config get|set|unset`) for a
/// Query/Mutate verb off the tokio runtime (ADR-0036 §2): unlike the streaming
/// Spawn path, a config verb yields ONE collected reply. It waits for one of the
/// shared command slots and answers within [`dispatch::REPLY_DEADLINE`]. Runs in
/// `cwd` with the dispatch `daemon_id`.
pub(crate) async fn collect_config(
    argv: Vec<String>,
    cwd: PathBuf,
    daemon_id: Option<String>,
) -> dispatch::Collected {
    dispatch::collect_within(
        std::sync::Arc::new(dispatch::ProcessSpawner),
        dispatch::ralphy_exe(),
        argv,
        cwd,
        daemon_id,
        dispatch::COLLECT_SLOTS.clone(),
        dispatch::REPLY_DEADLINE,
    )
    .await
}

/// Resolve the optional `checkout` key of `cmd` against `repo_path`. `Err` is
/// the reply to send in its place: `unknown checkout` for a name that does not
/// resolve (the text the shell drops its selection on), `unavailable` when the
/// pointer-file read could not run. The read is a filesystem read like every
/// Observe sibling: off the runtime, so a cold `/mnt/c` never parks a worker.
pub(crate) async fn checkout_of(
    cmd: &protocol::Command,
    repo_path: &Path,
) -> Result<Option<checkout::Checkout>, serde_json::Value> {
    let (payload, root) = (cmd.payload.clone(), repo_path.to_path_buf());
    match blocking_read(move || checkout::from_payload(&payload, &root)).await {
        Some(Ok(c)) => Ok(c),
        Some(Err(e)) => Err(serde_json::json!({ "status": "error", "message": e.to_string() })),
        None => Err(serde_json::json!({ "status": "error", "reason": "unavailable" })),
    }
}

/// The `current_dir` of a spawn-and-collect verb: the selected worktree for the
/// git-backed family (`Verb::takes_checkout_cwd`), the registry path for every
/// other verb — which never reads the key, so it never answers `unknown
/// checkout` either. Resolved AFTER the argv composed (a malformed param keeps
/// its "invalid … options" reply) and BEFORE any spawn. The worktree dir is
/// confined against the registered root exactly as the searches confine their
/// walk root: a `.ralphy/worktrees/<name>` that is a symlink out of the repo
/// is `unknown checkout` here too, never a cwd for `changes discard`.
pub(crate) async fn spawn_cwd(
    verb: dispatch::Verb,
    cmd: &protocol::Command,
    repo_path: &Path,
) -> Result<PathBuf, serde_json::Value> {
    if !verb.takes_checkout_cwd() {
        return Ok(repo_path.to_path_buf());
    }
    let Some(c) = checkout_of(cmd, repo_path).await? else {
        return Ok(repo_path.to_path_buf());
    };
    let (root, rel) = (repo_path.to_path_buf(), c.prefix(""));
    match blocking_read(move || confine::confine(&root, &rel)).await {
        Some(Ok(_)) => Ok(c.dir(repo_path)),
        Some(Err(_)) => Err(serde_json::json!({ "status": "error", "message": checkout::UNKNOWN })),
        None => Err(serde_json::json!({ "status": "error", "reason": "unavailable" })),
    }
}

/// Answer one non-streaming verb by effect class. The optional `checkout`
/// argument (ADR-0036 `checkout` amendment, ADR-0063 §2) is read by each class
/// that honours it, and becomes a rel PREFIX under the same registered root
/// (`checkout::from_payload`), so confinement never learns a second root. The
/// git-backed verbs (`Verb::takes_checkout_cwd`) run their composed command
/// with the worktree as `current_dir` (ADR-0063 §2) — the argv is unchanged; a
/// worktree act is neither held by nor holds the primary's run lock (the
/// worktree has no `.ralphy/`; the lock is the primary tree's).
/// `worktree.remove` is gated before its argv is composed (ADR-0063 §2): while
/// a live session of `slug` was spawned in that worktree, the reply is `has a
/// live console` and nothing is spawned — the session table is the daemon's
/// alone, so the CLI cannot apply this gate.
pub(crate) async fn execute_oneshot(
    verb: dispatch::Verb,
    cmd: &protocol::Command,
    repo_path: &Path,
    daemon_id: Option<&str>,
    slug: &str,
    sessions: &session::SessionManager,
) -> Option<serde_json::Value> {
    match verb.effect_class() {
        dispatch::EffectClass::Observe => Some(observe::observe(verb, cmd, repo_path).await),
        dispatch::EffectClass::Write => Some(write::write(verb, cmd, repo_path).await),
        dispatch::EffectClass::Query => Some(collect::query(verb, cmd, repo_path, daemon_id).await),
        dispatch::EffectClass::Mutate => {
            Some(collect::mutate(verb, cmd, repo_path, daemon_id, slug, sessions).await)
        }
        dispatch::EffectClass::Spawn | dispatch::EffectClass::Native => None,
    }
}

/// Answer `note.write` (ADR-0064 §5): the markdown the card holds is packed
/// into a `.note` container and written to the client's `path`, under the
/// selected checkout's root when one was named.
///
/// The client names a path here — unlike the clipboard drop, whose name the
/// daemon chooses — because a note is the operator's own document and §4 lets
/// them keep it wherever they back it up. What the VERB fixes instead is the
/// target CLASS: only a `.note`, and only bytes this daemon encoded. Every
/// other guard is the generic one — [`note::write`] goes through the same
/// confinement and denylist as [`fswrite`], whose single carve-out is the
/// notes landing directory.
///
/// An absent `markdown` is a refusal, never an empty save: a malformed payload
/// must not be the way a note is emptied.
async fn note_write(cmd: &protocol::Command, repo_path: &Path) -> serde_json::Value {
    let checkout = match checkout_of(cmd, repo_path).await {
        Ok(c) => c,
        Err(reply) => return reply,
    };
    let Some(markdown) = cmd.payload.get("markdown").and_then(|v| v.as_str()) else {
        return serde_json::json!({
            "status": "error",
            "reason": "refused",
            "message": "A note write needs its Markdown.",
        });
    };
    let rel = cmd
        .payload
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // The worktree dir is confined against the REGISTERED root before it is
    // used as one, exactly as `spawn_cwd` confines it: a
    // `.ralphy/worktrees/<name>` that is a symlink out of the repo is
    // `unknown checkout` here too, never a root a write confines against.
    let root = match &checkout {
        None => repo_path.to_path_buf(),
        Some(c) => {
            let (base, prefix) = (repo_path.to_path_buf(), c.prefix(""));
            match blocking_read(move || confine::confine(&base, &prefix)).await {
                Some(Ok(_)) => c.dir(repo_path),
                Some(Err(_)) => {
                    return serde_json::json!({
                        "status": "error",
                        "message": checkout::UNKNOWN,
                    })
                }
                None => return serde_json::json!({ "status": "error", "reason": "unavailable" }),
            }
        }
    };
    let markdown = markdown.to_string();
    match blocking_read(move || note::write(&root, &rel, &markdown)).await {
        Some(Ok(())) => serde_json::json!({ "status": "ok" }),
        Some(Err(e)) => serde_json::json!({ "status": "error", "reason": e.reason() }),
        None => serde_json::json!({ "status": "error", "reason": "unavailable" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_late_replies_name_the_real_limits() {
        let seconds = format!("after {} seconds", dispatch::REPLY_DEADLINE.as_secs());
        assert!(STILL_RUNNING.contains(&seconds), "{STILL_RUNNING}");
        let slots = format!(
            "{} commands are still running",
            dispatch::MAX_COLLECT_CHILDREN
        );
        assert!(NO_SLOT.contains(&slots), "{NO_SLOT}");
    }
}
