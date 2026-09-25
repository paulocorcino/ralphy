//! `/ws/command`: the verb socket — spawn-and-stream, spawn-and-collect and
//! the Observe verbs, or the relay of a verb to the owning peer (ADR-0036).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};

mod oneshot;

pub(crate) use oneshot::*;

use super::{read_peer_store, send_command};
use crate::protocol::{Command, Frame};
use crate::{dispatch, fleet, peer, protocol, registry, session};

/// `GET /ws/command`: one remote command per connection. Read the first frame; a
/// `Frame::Command{verb}` naming a blessed [`dispatch::Verb`] for a registered
/// repo spawns the run and reports its lifecycle — an ack (`status:"spawned"` +
/// pid), a stream of live output (`status:"output"` + `chunk`, issue #180), then
/// the child's exit (`status:"exited"` + code). An unknown verb or an unregistered
/// repo gets one `status:"error"` frame and spawns nothing.
///
/// TEARDOWN INVARIANT (the INVERSE of `session_ws`): the dispatched run keeps its
/// OWN lifecycle. NONE of the `select!` arms — daemon shutdown, client
/// close/error, output, wait-complete — kills the child; the
/// `Box<dyn dispatch::Child>` has no kill and dropping it does not kill (std
/// semantics). A daemon shutdown or a browser disconnect stops us serving THIS
/// socket but never the run (PRD #157 story 18/20). Do not add a kill to any arm.
/// The output DRAIN task is likewise detached: it reads the child's pipe to EOF
/// regardless of client presence, so a disconnect never stalls the child on a
/// full pipe. Do not await it on a teardown arm.
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
) {
    // First frame or nothing: a client that opens and hangs up spawns nothing.
    // A frame that is refused (too big for the socket's limits, not binary,
    // undecodable) drops the connection without a reply — the browser then
    // reports only "closed before a reply", so the reason is logged HERE, the
    // one place that knows it.
    let bytes = match socket.recv().await {
        Some(Ok(Message::Binary(bytes))) => bytes,
        Some(Ok(Message::Close(_))) | None => return,
        Some(Ok(_)) => {
            tracing::warn!("command socket: first frame is not binary; dropping");
            return;
        }
        Some(Err(e)) => {
            tracing::warn!(error = %e, "command socket: could not read the first frame");
            return;
        }
    };
    let cmd = match protocol::decode(&bytes) {
        Ok(Frame::Command(cmd)) => cmd,
        Ok(_) => {
            tracing::warn!("command socket: first frame is not a command; dropping");
            return;
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                bytes = bytes.len(),
                "command socket: undecodable first frame"
            );
            return;
        }
    };
    let id = cmd.id;

    let Some(verb) = dispatch::Verb::from_query(&cmd.verb) else {
        send_command(
            &mut socket,
            id,
            &cmd.verb,
            serde_json::json!({ "status": "error", "message": "unknown verb" }),
        )
        .await;
        return;
    };
    let repo_ref = cmd
        .payload
        .get("repo")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let (descriptors, _) = read_peer_store(peers_dir).await;
    let slug = match fleet::route(repo_ref, daemon_id.as_deref().unwrap_or(""), &descriptors) {
        fleet::Route::Local { slug } => slug.to_string(),
        fleet::Route::UnknownDaemon { daemon_id } => {
            send_command(
                &mut socket,
                id,
                &cmd.verb,
                serde_json::json!({
                    "status": "error",
                    "message": format!(
                        "No environment is announced as {daemon_id}. Its daemon has not written a peer descriptor into this store."
                    ),
                }),
            )
            .await;
            return;
        }
        fleet::Route::Peer { peer, slug } => {
            if verb.effect_class() == dispatch::EffectClass::Spawn {
                let mut proxied = cmd.clone();
                proxied.payload["repo"] = serde_json::Value::String(slug.to_string());
                proxy_peer_command(
                    &mut socket,
                    peer,
                    proxied,
                    &mut shutdown,
                    peer::client::SelfRef {
                        port: bound_port,
                        daemon_id: daemon_id.as_deref().unwrap_or(""),
                    },
                )
                .await;
                return;
            }
            let mut proxied = cmd.clone();
            proxied.payload["repo"] = serde_json::Value::String(slug.to_string());
            let body = serde_json::to_value(&proxied).expect("Command always serializes");
            let payload = match peer::client::post_json_timeout(
                peer,
                "/api/peer/command",
                &body,
                Duration::from_secs(60),
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
                    "message": fleet::peer_unreachable(peer, &format!("{e:#}")),
                }),
            };
            send_command(&mut socket, id, &cmd.verb, payload).await;
            return;
        }
    };
    let store = match registry::load_from(&registry_path) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load repo registry for a command");
            send_command(
                &mut socket,
                id,
                &cmd.verb,
                serde_json::json!({ "status": "error", "message": "repo registry unreadable" }),
            )
            .await;
            return;
        }
    };
    let Some(entry) = store.entry(&slug) else {
        send_command(
            &mut socket,
            id,
            &cmd.verb,
            serde_json::json!({ "status": "error", "message": "unknown repo" }),
        )
        .await;
        return;
    };

    if let Some(payload) = execute_oneshot(
        verb,
        &cmd,
        Path::new(&entry.path),
        daemon_id.as_deref(),
        &slug,
        &sessions,
    )
    .await
    {
        send_command(&mut socket, id, &cmd.verb, payload).await;
        return;
    }

    // Compose the argv from the verb + closed-enum params (ADR-0036 §1). A
    // malformed/out-of-enum param refuses the run: one error frame, no spawn.
    let argv = match dispatch::spawn_argv(verb, &cmd.payload) {
        Ok(argv) => argv,
        Err(e) => {
            tracing::warn!(error = %e, "refused a run with invalid params");
            send_command(
                &mut socket,
                id,
                &cmd.verb,
                serde_json::json!({ "status": "error", "message": "invalid run options" }),
            )
            .await;
            return;
        }
    };
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let mut child = match dispatch::dispatch(
        &dispatch::ProcessSpawner,
        &dispatch::ralphy_exe(),
        &argv_refs,
        Path::new(&entry.path),
        daemon_id.as_deref(),
    ) {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!(error = %e, "failed to spawn a dispatched command");
            send_command(
                &mut socket,
                id,
                &cmd.verb,
                serde_json::json!({ "status": "error", "message": "spawn failed" }),
            )
            .await;
            return;
        }
    };
    let pid = child.pid();
    // Take the merged output reader BEFORE `child` moves into the wait task, so
    // the drain and the wait run concurrently (each owns its half).
    let output = child.take_output();
    send_command(
        &mut socket,
        id,
        &cmd.verb,
        serde_json::json!({ "status": "spawned", "pid": pid }),
    )
    .await;

    // A DETACHED drain owns the reader and reads to EOF unconditionally — never
    // awaited on a teardown arm, so a client disconnect never stops it and the
    // child never stalls on a full pipe (see dispatch.rs OUTPUT STREAMING). A
    // dropped receiver only makes `send` error, which the drain IGNORES.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    if let Some(mut reader) = output {
        tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        let _ = tx.send(buf[..n].to_vec());
                    }
                }
            }
        });
    }

    // `Child::wait` is blocking and must not sit on the tokio runtime.
    //
    // The run-completion nudge (#310) is sent HERE, inside the blocking task, and
    // not from the wait arm below: the shutdown and client-disconnect arms `break`
    // without ever polling that arm while the run keeps living (the teardown
    // invariant above), and tokio never cancels a blocking task — so this is the
    // only site that fires on every exit path the daemon outlives (a run that
    // outlives the process has no nudge to send — the browser's reconnect
    // catch-up read covers that one). A send with no `/ws/tree`
    // subscriber is `Err`, and a nudge nobody hears is a no-op (as in `watch.rs`).
    let nudge_slug = slug.to_string();
    let mut wait = tokio::task::spawn_blocking(move || {
        let result = child.wait();
        let _ = run_exits.send(nudge_slug);
        result
    });
    // Disables the output arm once the drain channel closes (child pipe EOF), so
    // a closed `rx` never busy-loops and the other arms keep being polled.
    let mut output_open = true;
    loop {
        tokio::select! {
            // Daemon shutdown: stop serving this socket, but LEAVE the run alive.
            _ = shutdown.changed() => break,
            // Client closed or errored: same — abandon the wait, never kill.
            incoming = socket.recv() => {
                let _ = incoming;
                break;
            }
            // A live output chunk: forward it into the UI log pane.
            chunk = rx.recv(), if output_open => {
                match chunk {
                    Some(chunk) => {
                        send_command(
                            &mut socket,
                            id,
                            &cmd.verb,
                            serde_json::json!({
                                "status": "output",
                                "chunk": String::from_utf8_lossy(&chunk),
                            }),
                        )
                        .await;
                    }
                    // Drain closed (child pipe EOF): stop polling this arm and let
                    // the wait arm report the exit.
                    None => output_open = false,
                }
            }
            // The run exited: flush remaining output before the exit frame.
            // `recv().await` (not `try_recv`) closes the trailing-output race —
            // `wait` returns before the drain thread has forwarded the child's
            // final bytes. But the drain reaches EOF (and drops `tx`) only when
            // EVERY pipe write end is closed, and a `ralphy run` DESCENDANT can
            // inherit the merged fds and outlive the primary child — so we bound
            // the wait for each next chunk: an idle gap (or channel close) ends
            // the flush and we always emit `exited`, never wedging the handler.
            joined = &mut wait => {
                while let Ok(Some(chunk)) =
                    tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
                {
                    send_command(
                        &mut socket,
                        id,
                        &cmd.verb,
                        serde_json::json!({
                            "status": "output",
                            "chunk": String::from_utf8_lossy(&chunk),
                        }),
                    )
                    .await;
                }
                let code = joined.ok().and_then(|r| r.ok()).flatten();
                send_command(
                    &mut socket,
                    id,
                    &cmd.verb,
                    serde_json::json!({ "status": "exited", "code": code }),
                )
                .await;
                break;
            }
        }
    }
}

pub(crate) async fn proxy_peer_command(
    browser: &mut WebSocket,
    descriptor: &peer::PeerDescriptor,
    command: Command,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    me: peer::client::SelfRef<'_>,
) {
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
            return;
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
            return;
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
        return;
    }

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
                match message {
                    tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
                        let terminal = matches!(
                            protocol::decode(&bytes),
                            Ok(Frame::Command(ref reply))
                                if reply.id == command.id
                                    && matches!(
                                        reply.payload.get("status").and_then(|value| value.as_str()),
                                        Some("exited" | "error")
                                    )
                        );
                        if browser.send(Message::Binary(bytes)).await.is_err() || terminal {
                            break;
                        }
                    }
                    tokio_tungstenite::tungstenite::Message::Ping(bytes) => {
                        if browser.send(Message::Ping(bytes)).await.is_err() {
                            break;
                        }
                    }
                    tokio_tungstenite::tungstenite::Message::Pong(bytes) => {
                        if browser.send(Message::Pong(bytes)).await.is_err() {
                            break;
                        }
                    }
                    tokio_tungstenite::tungstenite::Message::Close(_)
                    | tokio_tungstenite::tungstenite::Message::Text(_)
                    | tokio_tungstenite::tungstenite::Message::Frame(_) => continue,
                }
            }
        }
    }
}
