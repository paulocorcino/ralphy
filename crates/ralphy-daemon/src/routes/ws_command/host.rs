//! The host verbs on `/ws/command` (ADR-0036 amendment 2026-09-29, ADR-0067):
//! `ralphy host …` on the computer this daemon runs on. They name no repo, so
//! they are served before any routing, and never relayed to a peer.

use std::path::Path;

use axum::extract::ws::WebSocket;

use super::{collect_config, not_answered, send_command, store_dir, stream, CommandCtx};
use crate::dispatch::{self, Verb};
use crate::protocol::Command;

/// A host command that passed its checks: the password, if any, and the argv.
type Checked = (Option<zeroize::Zeroizing<String>>, Vec<String>);

/// The checks every host verb passes before it runs: a valid password field,
/// a password only over a channel that may carry a secret, and valid params.
/// `Err` is the one error reply.
fn check(cmd: &Command, verb: Verb, secret_ok: bool) -> Result<Checked, serde_json::Value> {
    let password = match dispatch::host_password(verb, &cmd.payload) {
        Ok(password) => password,
        Err(e) => {
            tracing::warn!(error = %e, "refused a host command with an invalid password field");
            return Err(
                serde_json::json!({ "status": "error", "message": "invalid host options" }),
            );
        }
    };
    if password.is_some() && !secret_ok {
        tracing::warn!("refused a host password that came over plain http from the network");
        return Err(serde_json::json!({
            "status": "error",
            "message": "A password is accepted only over https or from this computer. Use a key, or open the workbench over https.",
        }));
    }
    match dispatch::host_argv(verb, &cmd.payload) {
        Ok(argv) => Ok((password, argv)),
        Err(e) => {
            tracing::warn!(error = %e, "refused a host command with invalid params");
            Err(serde_json::json!({ "status": "error", "message": "invalid host options" }))
        }
    }
}

/// The refusal a host verb gets before it runs, or `None` when it passes.
pub(crate) fn host_refusal(
    cmd: &Command,
    verb: Verb,
    secret_ok: bool,
) -> Option<serde_json::Value> {
    check(cmd, verb, secret_ok).err()
}

/// The one reply to a Query or Mutate host verb.
pub(super) async fn host_reply(
    cmd: &Command,
    verb: Verb,
    store_dir: &Path,
    daemon_id: Option<&str>,
    secret_ok: bool,
) -> serde_json::Value {
    match check(cmd, verb, secret_ok) {
        Ok((_, argv)) => collect_reply(verb, argv, store_dir, daemon_id).await,
        Err(reply) => reply,
    }
}

/// Spawn a Spawn host verb and stream its run. A refusal is one error frame.
pub(super) async fn spawn_host(
    socket: &mut WebSocket,
    cmd: &Command,
    verb: Verb,
    ctx: &CommandCtx,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) {
    let store_dir = store_dir(&ctx.peers_dir);
    let store_dir = store_dir.as_path();
    let daemon_id = ctx.daemon_id.as_deref();
    let (password, argv) = match check(cmd, verb, ctx.secret_ok) {
        Ok(checked) => checked,
        Err(reply) => {
            send_command(socket, cmd.id, &cmd.verb, reply).await;
            return;
        }
    };
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let spawned = match &password {
        Some(password) => dispatch::ProcessSpawner.spawn_with_input(
            &dispatch::ralphy_exe(),
            &argv_refs,
            store_dir,
            daemon_id,
            password.as_bytes(),
        ),
        None => dispatch::dispatch(
            &dispatch::ProcessSpawner,
            &dispatch::ralphy_exe(),
            &argv_refs,
            store_dir,
            daemon_id,
        ),
    };
    drop(password);
    let child = match spawned {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!(error = %e, "failed to spawn a host command");
            let reply = serde_json::json!({ "status": "error", "message": "spawn failed" });
            send_command(socket, cmd.id, &cmd.verb, reply).await;
            return;
        }
    };
    stream::stream_child(socket, cmd.id, &cmd.verb, child, shutdown, || {}).await;
}

/// Run a Query or Mutate host verb to its end. A Query's JSON stdout goes
/// under the verb's field; a Mutate answers `ok`. A failure relays the
/// command's own output.
async fn collect_reply(
    verb: Verb,
    argv: Vec<String>,
    store_dir: &Path,
    daemon_id: Option<&str>,
) -> serde_json::Value {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "only a host verb reaches this function and `Verb` has over 60 variants"
    )]
    let field = match verb {
        Verb::HostAliases => Some("aliases"),
        Verb::HostKey => Some("key"),
        _ => None,
    };
    match collect_config(argv, store_dir.to_path_buf(), daemon_id.map(str::to_owned)).await {
        dispatch::Collected::Done(Some(0), bytes) => {
            let Some(field) = field else {
                return serde_json::json!({ "status": "ok" });
            };
            let text = String::from_utf8_lossy(&bytes);
            // `collect` merges stderr into stdout; the JSON is the last line.
            let last = text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            match serde_json::from_str::<serde_json::Value>(last.trim()) {
                Ok(parsed) => {
                    let mut obj = serde_json::Map::new();
                    obj.insert("status".to_string(), serde_json::json!("ok"));
                    obj.insert(field.to_string(), parsed);
                    serde_json::Value::Object(obj)
                }
                Err(e) => {
                    tracing::warn!(error = %e, verb = ?verb, "a host query printed no JSON");
                    serde_json::json!({ "status": "error", "message": text.trim() })
                }
            }
        }
        dispatch::Collected::Done(_, bytes) => serde_json::json!({
            "status": "error",
            "message": String::from_utf8_lossy(&bytes).trim(),
        }),
        dispatch::Collected::Failed(e) => {
            tracing::warn!(error = %format!("{e:#}"), verb = ?verb, "a host command failed to run");
            serde_json::json!({ "status": "error", "message": "host command failed to run" })
        }
        late @ (dispatch::Collected::StillRunning | dispatch::Collected::NoSlot) => {
            not_answered(&late).expect("a late answer has a reply")
        }
    }
}
