//! The host verbs on `/ws/command` (ADR-0036 amendment 2026-09-29, ADR-0067):
//! `ralphy host …` on the computer this daemon runs on. They name no repo, so
//! they are served before any routing, and never relayed to a peer.

use std::path::Path;

use axum::extract::ws::WebSocket;

use super::{collect_config, send_command, stream, STILL_RUNNING};
use crate::dispatch::{self, EffectClass, Verb};
use crate::protocol::Command;

pub(super) async fn serve_host(
    socket: &mut WebSocket,
    cmd: &Command,
    verb: Verb,
    store_dir: &Path,
    daemon_id: Option<&str>,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    secret_ok: bool,
) {
    let password = match dispatch::host_password(verb, &cmd.payload) {
        Ok(password) => password,
        Err(e) => {
            tracing::warn!(error = %e, "refused a host command with an invalid password field");
            let reply = serde_json::json!({ "status": "error", "message": "invalid host options" });
            send_command(socket, cmd.id, &cmd.verb, reply).await;
            return;
        }
    };
    if password.is_some() && !secret_ok {
        tracing::warn!("refused a host password that came over plain http from the network");
        let reply = serde_json::json!({
            "status": "error",
            "message": "A password is accepted only over https or from this computer. Use a key, or open the workbench over https.",
        });
        send_command(socket, cmd.id, &cmd.verb, reply).await;
        return;
    }
    let argv = match dispatch::host_argv(verb, &cmd.payload) {
        Ok(argv) => argv,
        Err(e) => {
            tracing::warn!(error = %e, "refused a host command with invalid params");
            let reply = serde_json::json!({ "status": "error", "message": "invalid host options" });
            send_command(socket, cmd.id, &cmd.verb, reply).await;
            return;
        }
    };
    if verb.effect_class() != EffectClass::Spawn {
        let reply = collect_reply(verb, argv, store_dir, daemon_id).await;
        send_command(socket, cmd.id, &cmd.verb, reply).await;
        return;
    }
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
        dispatch::Collected::StillRunning => {
            serde_json::json!({ "status": "error", "message": STILL_RUNNING })
        }
    }
}
