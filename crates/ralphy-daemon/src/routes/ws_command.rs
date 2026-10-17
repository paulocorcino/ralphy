//! `/ws/command`: the verb socket — spawn-and-stream, spawn-and-collect and
//! the Observe verbs, or the relay of a verb to the owning peer (ADR-0036).

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};

mod host;
mod oneshot;
mod registry_verbs;
mod stream;
#[cfg(test)]
mod tests;

pub(crate) use oneshot::*;
pub(crate) use registry_verbs::serve_registry;

use super::audit_layer::SocketAudit;
use super::{read_peer_store, send_command};
use super::{request_may_carry_a_secret, RouterShared};
use crate::protocol::{Command, Frame};
use crate::{dispatch, fleet, peer, protocol, registry, session};

/// What the command socket needs to answer a one-reply command.
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

/// `GET /ws/command`: one remote command per connection. Read the first frame; a
/// `Frame::Command{verb}` naming a blessed [`dispatch::Verb`] for a registered
/// repo spawns the run and reports its lifecycle — an ack (`status:"spawned"` +
/// pid), a stream of live output (`status:"output"` + `chunk`, issue #180), then
/// the child's exit (`status:"exited"` + code). An unknown verb or an unregistered
/// repo gets one `status:"error"` frame and spawns nothing. A one-reply verb is
/// answered by [`answer_one`]. The run keeps its own lifecycle: see the teardown
/// invariant on [`stream::stream_child`].
// The router's per-route dependencies, one parameter each (precedent: `usage_route`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn command_ws(
    mut socket: WebSocket,
    registry_path: PathBuf,
    peers_dir: PathBuf,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    daemon_id: Option<String>,
    run_exits: tokio::sync::broadcast::Sender<String>,
    bound_port: u16,
    sessions: Arc<session::SessionManager>,
    secret_ok: bool,
    who: SocketAudit,
) {
    let Some(cmd) = read_first_command(&mut socket).await else {
        return;
    };
    let ctx = CommandCtx {
        registry_path,
        peers_dir,
        daemon_id,
        sessions,
        secret_ok,
    };
    let verb = match answer_one(&ctx, &cmd, &who).await {
        Answer::Reply(reply) => {
            send_command(&mut socket, cmd.id, &cmd.verb, reply).await;
            return;
        }
        Answer::Stream(verb) => verb,
    };
    let daemon_id = ctx.daemon_id.as_deref();
    if verb.is_host() {
        let store_dir = store_dir(&ctx.peers_dir);
        host::spawn_host(
            &mut socket,
            &cmd,
            verb,
            &store_dir,
            daemon_id,
            &mut shutdown,
            secret_ok,
        )
        .await;
        return;
    }
    let (descriptors, _) = read_peer_store(ctx.peers_dir.clone()).await;
    let slug = match fleet::route(repo_ref(&cmd), daemon_id.unwrap_or(""), &descriptors) {
        fleet::Route::Local { slug } => slug.to_string(),
        fleet::Route::UnknownDaemon { daemon_id } => {
            send_command(&mut socket, cmd.id, &cmd.verb, unknown_daemon(daemon_id)).await;
            return;
        }
        fleet::Route::Peer { peer, slug } => {
            let me = peer::client::SelfRef {
                port: bound_port,
                daemon_id: daemon_id.unwrap_or(""),
            };
            proxy_peer_command(&mut socket, peer, for_peer(&cmd, slug), &mut shutdown, me).await;
            return;
        }
    };
    let child = match spawn_local(&cmd, verb, &slug, &ctx.registry_path, daemon_id) {
        Ok(child) => child,
        Err(reply) => {
            send_command(&mut socket, cmd.id, &cmd.verb, reply).await;
            return;
        }
    };
    // The run-completion nudge (#310) rides on the child's exit, which
    // `stream_child` observes on every path the daemon outlives (a run that
    // outlives the process has no nudge to send — the browser's reconnect
    // catch-up read covers that one). A send with no `/ws/tree` subscriber is
    // `Err`, and a nudge nobody hears is a no-op (as in `watch.rs`).
    stream::stream_child(
        &mut socket,
        cmd.id,
        &cmd.verb,
        child,
        &mut shutdown,
        move || {
            let _ = run_exits.send(slug);
        },
    )
    .await;
}

/// The first frame of a command socket, as a command.
///
/// First frame or nothing: a client that opens and hangs up spawns nothing.
/// A frame that is refused (too big for the socket's limits, not binary,
/// undecodable) drops the connection without a reply — the browser then
/// reports only "closed before a reply", so the reason is logged HERE, the
/// one place that knows it.
async fn read_first_command(socket: &mut WebSocket) -> Option<Command> {
    let bytes = match socket.recv().await {
        Some(Ok(Message::Binary(bytes))) => bytes,
        Some(Ok(Message::Close(_))) | None => return None,
        Some(Ok(_)) => {
            tracing::warn!("command socket: first frame is not binary; dropping");
            return None;
        }
        Some(Err(e)) => {
            tracing::warn!(error = %e, "command socket: could not read the first frame");
            return None;
        }
    };
    match protocol::decode(&bytes) {
        Ok(Frame::Command(cmd)) => Some(cmd),
        Ok(_) => {
            tracing::warn!("command socket: first frame is not a command; dropping");
            None
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                bytes = bytes.len(),
                "command socket: undecodable first frame"
            );
            None
        }
    }
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

pub(crate) async fn proxy_peer_command(
    browser: &mut WebSocket,
    descriptor: &peer::PeerDescriptor,
    command: Command,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    me: peer::client::SelfRef<'_>,
) {
    let Some(mut peer_socket) = open_peer(browser, descriptor, &command, me).await else {
        return;
    };

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            incoming = browser.recv() => {
                let _ = incoming;
                break;
            }
            incoming = peer_socket.next() => {
                let Some(Ok(message)) = incoming else {
                    send_command(
                        browser,
                        command.id,
                        &command.verb,
                        serde_json::json!({
                            "status": "error",
                            "message": fleet::peer_unreachable(
                                descriptor,
                                "the peer command socket closed before a terminal frame"
                            ),
                        }),
                    )
                    .await;
                    break;
                };
                if forward_peer_message(browser, message, command.id).await.is_break() {
                    break;
                }
            }
        }
    }
}

/// Open the peer's command socket and send it `command`. `None` once the
/// browser has the one error frame that says why it failed.
async fn open_peer(
    browser: &mut WebSocket,
    descriptor: &peer::PeerDescriptor,
    command: &Command,
    me: peer::client::SelfRef<'_>,
) -> Option<peer::client::PeerSocket> {
    let mut peer_socket = match peer::client::command(descriptor, me).await {
        Ok(socket) => socket,
        Err(peer::client::SocketError::Peer(status)) => {
            send_command(
                browser,
                command.id,
                &command.verb,
                serde_json::json!({
                    "status": "error",
                    "message": status.diagnosis(&descriptor.environment),
                }),
            )
            .await;
            return None;
        }
        Err(peer::client::SocketError::Http { status, body }) => {
            let detail = if body.trim().is_empty() {
                format!("peer command upgrade answered HTTP {status}")
            } else {
                body
            };
            send_command(
                browser,
                command.id,
                &command.verb,
                serde_json::json!({
                    "status": "error",
                    "message": fleet::peer_unreachable(descriptor, &detail),
                }),
            )
            .await;
            return None;
        }
    };
    if peer_socket
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            protocol::encode(&Frame::Command(command.clone())).into(),
        ))
        .await
        .is_err()
    {
        send_command(
            browser,
            command.id,
            &command.verb,
            serde_json::json!({
                "status": "error",
                "message": fleet::peer_unreachable(
                    descriptor,
                    "the peer command socket closed before accepting the command"
                ),
            }),
        )
        .await;
        return None;
    }
    Some(peer_socket)
}

/// Forward one peer frame to the browser. `Break` when the browser did not
/// take it, or after the terminal reply to command `id`.
async fn forward_peer_message(
    browser: &mut WebSocket,
    message: tokio_tungstenite::tungstenite::Message,
    id: u64,
) -> ControlFlow<()> {
    let sent = match message {
        tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
            let terminal = is_terminal_reply(&bytes, id);
            if browser.send(Message::Binary(bytes)).await.is_err() || terminal {
                return ControlFlow::Break(());
            }
            return ControlFlow::Continue(());
        }
        tokio_tungstenite::tungstenite::Message::Ping(bytes) => {
            browser.send(Message::Ping(bytes)).await
        }
        tokio_tungstenite::tungstenite::Message::Pong(bytes) => {
            browser.send(Message::Pong(bytes)).await
        }
        tokio_tungstenite::tungstenite::Message::Close(_)
        | tokio_tungstenite::tungstenite::Message::Text(_)
        | tokio_tungstenite::tungstenite::Message::Frame(_) => return ControlFlow::Continue(()),
    };
    if sent.is_err() {
        ControlFlow::Break(())
    } else {
        ControlFlow::Continue(())
    }
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

/// `/ws/command`: the command socket.
pub(crate) fn command_routes(s: &RouterShared) -> Router {
    let registry = s.registry_path.clone();
    let peers = s.peers_dir.clone();
    // A dispatched run must survive daemon shutdown (inverse of the session
    // invariant), but the handler still watches `shutdown` to stop serving the
    // socket — it just never kills the child.
    let shutdown = s.shutdown.clone();
    // The daemon identity a dispatched child inherits as RALPHY_DAEMON_ID (#168).
    // Only the dispatch path passes it; session/console children get none.
    let daemon_id = s.daemon_id.clone();
    let run_exits = s.run_exits.clone();
    let bound_port = s.bound_port;
    let sessions = s.sessions.clone();
    let audit = s.audit.clone();
    Router::new().route(
        "/ws/command",
        get(
            move |ws: WebSocketUpgrade,
                  caller: Option<axum::Extension<super::audit_layer::Caller>>,
                  headers: axum::http::HeaderMap| {
                let ws = ws
                    .max_message_size(crate::tree::MAX_COMMAND_BYTES)
                    .max_frame_size(crate::tree::MAX_COMMAND_BYTES);
                let secret_ok = request_may_carry_a_secret(&headers);
                let who = SocketAudit::of(audit.clone(), caller);
                let registry_path = registry.clone();
                let shutdown = shutdown.clone();
                let daemon_id = daemon_id.clone();
                let run_exits = run_exits.clone();
                let peers_dir = peers.clone();
                let sessions = sessions.clone();
                async move {
                    ws.on_upgrade(move |socket| {
                        command_ws(
                            socket,
                            registry_path,
                            peers_dir,
                            shutdown,
                            daemon_id,
                            run_exits,
                            bound_port,
                            sessions,
                            secret_ok,
                            who,
                        )
                    })
                }
            },
        ),
    )
}
