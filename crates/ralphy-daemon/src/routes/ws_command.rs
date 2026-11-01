//! The verbs of the daemon, as the `spawn` channel of the page socket and
//! `POST /api/command` serve them: spawn-and-stream, spawn-and-collect and
//! the Observe verbs, or the relay of a verb to the owning peer (ADR-0036).

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

mod host;
mod oneshot;
mod registry_verbs;
mod stream;
#[cfg(test)]
mod tests;
mod wire;

pub(crate) use host::host_refusal;
pub(crate) use oneshot::*;
pub(crate) use registry_verbs::serve_registry;
pub(crate) use wire::Wire;

use super::audit_layer::SocketAudit;
use super::send_command_on;
use crate::peer::read_peer_store;
use crate::protocol::{Command, Frame};
use crate::{dispatch, fleet, peer, protocol, registry, session};

/// What serving a command needs: the stores, this daemon and its consoles.
pub(crate) struct CommandCtx {
    pub(crate) registry_path: PathBuf,
    pub(crate) peers_dir: PathBuf,
    pub(crate) daemon_id: Option<String>,
    pub(crate) sessions: Arc<session::SessionManager>,
    pub(crate) secret_ok: bool,
}

/// How a command is answered: one reply, or a run whose output streams.
#[derive(Debug)]
pub(crate) enum Answer {
    Reply(serde_json::Value),
    Stream(dispatch::Verb),
}

/// Answer `cmd` with its one reply, or say that its verb streams. Every
/// known verb is recorded once, here. A Spawn verb is never run here.
pub(crate) async fn answer_one(ctx: &CommandCtx, cmd: &Command, who: &SocketAudit) -> Answer {
    // Not a registry verb, and only in a developer build (ADR-0084 D2).
    #[cfg(feature = "scroll-probe")]
    if cmd.verb == crate::probe::VERB {
        let dir = store_dir(&ctx.peers_dir);
        return Answer::Reply(crate::probe::reply(cmd.payload.clone(), who.device, dir).await);
    }
    let Some(verb) = dispatch::Verb::from_query(&cmd.verb) else {
        return Answer::Reply(serde_json::json!({ "status": "error", "message": "unknown verb" }));
    };
    // Recorded before any routing: a verb relayed to a peer or refused here
    // was still asked for by this device.
    who.record(verb, cmd);
    if verb.effect_class() == dispatch::EffectClass::Spawn {
        return Answer::Stream(verb);
    }
    let daemon_id = ctx.daemon_id.as_deref();
    // A host verb names no repo and acts on THIS computer: served here, before
    // any repo routing, and never relayed to a peer.
    if verb.is_host() {
        let store_dir = store_dir(&ctx.peers_dir);
        let reply = host::host_reply(cmd, verb, &store_dir, daemon_id, ctx.secret_ok).await;
        return Answer::Reply(reply);
    }
    if verb.is_registry() {
        let peers_dir = ctx.peers_dir.clone();
        let reply = serve_registry_verb(cmd, verb, &ctx.registry_path, peers_dir, daemon_id).await;
        return Answer::Reply(reply);
    }
    let (descriptors, _) = read_peer_store(ctx.peers_dir.clone()).await;
    let reply = match fleet::route(repo_ref(cmd), daemon_id.unwrap_or(""), &descriptors) {
        fleet::Route::Local { slug } => local_reply(ctx, cmd, verb, slug).await,
        fleet::Route::UnknownDaemon { daemon_id } => unknown_daemon(daemon_id),
        fleet::Route::Peer { peer, slug } => relay_to_peer(peer, &for_peer(cmd, slug)).await,
    };
    who.record_upload(verb, cmd, &reply);
    Answer::Reply(reply)
}

/// The one reply to a one-shot repo verb served on this daemon.
async fn local_reply(
    ctx: &CommandCtx,
    cmd: &Command,
    verb: dispatch::Verb,
    slug: &str,
) -> serde_json::Value {
    let path = match repo_path(&ctx.registry_path, slug) {
        Ok(path) => path,
        Err(reply) => return reply,
    };
    let daemon_id = ctx.daemon_id.as_deref();
    // `None` only for an effect class no verb has today (`Native`).
    execute_oneshot(verb, cmd, &path, daemon_id, slug, &ctx.sessions)
        .await
        .unwrap_or_else(|| serde_json::json!({ "status": "error", "message": "unknown verb" }))
}

/// The checkout of the registered repo `slug`. `Err` is the one error reply.
fn repo_path(registry_path: &Path, slug: &str) -> Result<PathBuf, serde_json::Value> {
    let store = registry::load_from(registry_path).map_err(|e| {
        tracing::warn!(error = %e, "failed to load repo registry for a command");
        serde_json::json!({ "status": "error", "message": "repo registry unreadable" })
    })?;
    match store.entry_or_former(slug) {
        Some(entry) => Ok(PathBuf::from(&entry.path)),
        None => Err(serde_json::json!({ "status": "error", "message": "unknown repo" })),
    }
}

/// The store directory: the parent of the peers directory.
fn store_dir(peers_dir: &Path) -> PathBuf {
    peers_dir.parent().unwrap_or(peers_dir).to_path_buf()
}

fn repo_ref(cmd: &Command) -> &str {
    cmd.payload
        .get("repo")
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// `cmd` as the peer sees it: its repo is the peer's own slug.
fn for_peer(cmd: &Command, slug: &str) -> Command {
    let mut proxied = cmd.clone();
    proxied.payload["repo"] = serde_json::Value::String(slug.to_string());
    proxied
}

/// Answer `cmd` on `wire`. A Spawn verb for a registered repo spawns the run
/// and reports its lifecycle: an ack (`status:"spawned"` + pid), the live
/// output (`status:"output"` + `chunk`, issue #180), then the child's exit
/// (`status:"exited"` + code). An unknown verb or an unregistered repo gets
/// one `status:"error"` frame and spawns nothing. A one-reply verb is
/// answered by [`answer_one`]. The run keeps its own lifecycle: see the
/// teardown invariant on [`stream::stream_child`]. Every path returns to the
/// caller, which closes.
pub(crate) async fn serve_command(
    wire: &mut Wire,
    cmd: Command,
    ctx: CommandCtx,
    run_exits: tokio::sync::broadcast::Sender<String>,
    bound_port: u16,
    who: SocketAudit,
) {
    let verb = match answer_one(&ctx, &cmd, &who).await {
        Answer::Reply(reply) => {
            wire.reply(cmd.id, &cmd.verb, reply).await;
            return;
        }
        Answer::Stream(verb) => verb,
    };
    let daemon_id = ctx.daemon_id.as_deref();
    if verb.is_host() {
        host::spawn_host(wire, &cmd, verb, &ctx).await;
        return;
    }
    let (descriptors, _) = read_peer_store(ctx.peers_dir.clone()).await;
    let slug = match fleet::route(repo_ref(&cmd), daemon_id.unwrap_or(""), &descriptors) {
        fleet::Route::Local { slug } => slug.to_string(),
        fleet::Route::UnknownDaemon { daemon_id } => {
            wire.reply(cmd.id, &cmd.verb, unknown_daemon(daemon_id))
                .await;
            return;
        }
        fleet::Route::Peer { peer, slug } => {
            let me = peer::client::SelfRef {
                port: bound_port,
                daemon_id: daemon_id.unwrap_or(""),
            };
            proxy_peer_command(wire, peer, for_peer(&cmd, slug), me).await;
            return;
        }
    };
    let child = match spawn_local(&cmd, verb, &slug, &ctx.registry_path, daemon_id) {
        Ok(child) => child,
        Err(reply) => {
            wire.reply(cmd.id, &cmd.verb, reply).await;
            return;
        }
    };
    // The run-completion nudge (#310) rides on the child's exit, which
    // `stream_child` observes on every path the daemon outlives (a run that
    // outlives the process has no nudge to send — the browser's reconnect
    // catch-up read covers that one). A send with no `changes` channel to hear it is
    // `Err`, and a nudge nobody hears is a no-op (as in `watch.rs`).
    stream::stream_child(wire, cmd.id, &cmd.verb, child, move || {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a nudge nobody hears is a no-op"
        )]
        let _ = run_exits.send(slug);
    })
    .await;
}

/// The reply to a registry verb. A registry verb names a daemon, not a repo:
/// no `daemon`, or this daemon's own id, is served here; a peer's id is
/// relayed to that peer, which serves it against its own disk and registry.
async fn serve_registry_verb(
    cmd: &Command,
    verb: dispatch::Verb,
    registry_path: &Path,
    peers_dir: PathBuf,
    daemon_id: Option<&str>,
) -> serde_json::Value {
    let target = cmd.payload.get("daemon").and_then(|v| v.as_str());
    match target {
        None | Some("") => serve_registry(cmd, verb, registry_path, daemon_id).await,
        Some(t) if Some(t) == daemon_id => {
            serve_registry(cmd, verb, registry_path, daemon_id).await
        }
        Some(t) => {
            let (descriptors, _) = read_peer_store(peers_dir).await;
            match descriptors.iter().find(|d| d.daemon_id == t) {
                Some(peer) => {
                    let mut proxied = cmd.clone();
                    if let Some(obj) = proxied.payload.as_object_mut() {
                        obj.remove("daemon");
                    }
                    relay_to_peer(peer, &proxied).await
                }
                None => unknown_daemon(t),
            }
        }
    }
}

/// Spawn a Spawn repo verb on this daemon; its run is returned to be
/// streamed. `Err` is the one error reply: a refusal spawns nothing.
fn spawn_local(
    cmd: &Command,
    verb: dispatch::Verb,
    slug: &str,
    registry_path: &Path,
    daemon_id: Option<&str>,
) -> Result<Box<dyn dispatch::Child>, serde_json::Value> {
    let path = repo_path(registry_path, slug)?;
    // Compose the argv from the verb + closed-enum params (ADR-0036 §1). A
    // malformed/out-of-enum param refuses the run: one error frame, no spawn.
    let argv = dispatch::spawn_argv(verb, &cmd.payload).map_err(|e| {
        tracing::warn!(error = %e, "refused a run with invalid params");
        serde_json::json!({ "status": "error", "message": "invalid run options" })
    })?;
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let exe = dispatch::ralphy_exe();
    dispatch::dispatch(
        &dispatch::ProcessSpawner,
        &exe,
        &argv_refs,
        &path,
        daemon_id,
    )
    .map_err(|e| {
        tracing::warn!(error = %e, "failed to spawn a dispatched command");
        serde_json::json!({ "status": "error", "message": "spawn failed" })
    })
}

fn unknown_daemon(daemon_id: &str) -> serde_json::Value {
    serde_json::json!({
        "status": "error",
        "message": format!(
            "No environment is announced as {daemon_id}. Its daemon has not written a peer descriptor into this store."
        ),
    })
}

/// Relay a one-reply command to a peer's `/api/peer/command` and return the
/// peer's reply, or an error that names the peer.
async fn relay_to_peer(peer: &peer::PeerDescriptor, command: &Command) -> serde_json::Value {
    let body = serde_json::to_value(command).expect("Command always serializes");
    match peer::client::post_json_timeout(
        peer,
        "/api/peer/command",
        &body,
        // The peer answers its own "still running" at its deadline;
        // wait past it so that answer, not a transport timeout, arrives.
        dispatch::REPLY_DEADLINE + Duration::from_secs(5),
    )
    .await
    {
        Ok((200, body)) => serde_json::from_slice(&body).unwrap_or_else(|_| {
            serde_json::json!({
                "status": "error",
                "message": fleet::peer_unreachable(
                    peer,
                    "the peer answered invalid repo command data"
                ),
            })
        }),
        Ok((code, _)) => serde_json::json!({
            "status": "error",
            "message": fleet::peer_unreachable(
                peer,
                &format!("the peer answered HTTP {code} to a repo command")
            ),
        }),
        Err(e) => serde_json::json!({
            "status": "error",
            "message": peer::client::transport_failed(peer, format!("{e:#}")).await,
        }),
    }
}

/// Relay a Spawn verb to the peer that owns its repo, over a `spawn`
/// channel of the peer's page socket. Each whole frame of the peer goes to
/// the browser; the relay ends after the terminal reply to the command, when
/// the browser leaves, or with one error reply when the peer ends first.
pub(crate) async fn proxy_peer_command(
    browser: &mut Wire,
    descriptor: &peer::PeerDescriptor,
    command: Command,
    me: peer::client::SelfRef<'_>,
) {
    let Some(mut channel) = open_peer(browser, descriptor, &command, me).await else {
        return;
    };
    loop {
        tokio::select! {
            () = browser.gone() => break,
            event = channel.next() => {
                if relay_event(browser, descriptor, &command, event).await.is_break() {
                    break;
                }
            }
        }
    }
    channel.close().await;
}

/// Pass one event of the peer's channel to the browser. `Break` when the
/// browser did not take a frame, after the terminal reply to `command`, or
/// once the browser has the error reply for a peer that ended first.
async fn relay_event(
    browser: &Wire,
    descriptor: &peer::PeerDescriptor,
    command: &Command,
    event: peer::client::PeerEvent,
) -> ControlFlow<()> {
    match event {
        peer::client::PeerEvent::Frame(inner) => {
            let terminal = is_terminal_reply(&inner, command.id);
            if browser.out.send(inner).await.is_err() || terminal {
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        }
        peer::client::PeerEvent::Closed { .. } | peer::client::PeerEvent::Gone => {
            let reply = serde_json::json!({
                "status": "error",
                "message": fleet::peer_unreachable(
                    descriptor,
                    "the peer command socket closed before a terminal frame"
                ),
            });
            browser.reply(command.id, &command.verb, reply).await;
            ControlFlow::Break(())
        }
    }
}

/// Open a `spawn` channel on the peer's page socket, with `command` in the
/// open. `None` once the browser has the one error frame that says why it
/// failed.
async fn open_peer(
    browser: &Wire,
    descriptor: &peer::PeerDescriptor,
    command: &Command,
    me: peer::client::SelfRef<'_>,
) -> Option<peer::client::PeerChannel> {
    let fields = serde_json::json!({ "command": command });
    let message = match peer::client::open_channel(descriptor, me, "spawn", fields).await {
        Ok(channel) => return Some(channel),
        Err(peer::client::SocketError::Peer(status)) => status.diagnosis(&descriptor.environment),
        Err(peer::client::SocketError::Http { status, body }) => {
            let detail = if body.trim().is_empty() {
                format!("peer command upgrade answered HTTP {status}")
            } else {
                body
            };
            fleet::peer_unreachable(descriptor, &detail)
        }
    };
    let reply = serde_json::json!({ "status": "error", "message": message });
    browser.reply(command.id, &command.verb, reply).await;
    None
}

/// Whether `bytes` is the reply that ends command `id`: its `exited` or
/// `error` frame.
fn is_terminal_reply(bytes: &[u8], id: u64) -> bool {
    matches!(
        protocol::decode(bytes),
        Ok(Frame::Command(ref reply))
            if reply.id == id
                && matches!(
                    reply.payload.get("status").and_then(|value| value.as_str()),
                    Some("exited" | "error")
                )
    )
}
