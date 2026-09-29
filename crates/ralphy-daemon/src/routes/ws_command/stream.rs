//! The streaming tail shared by the Spawn verbs: relay a dispatched child's
//! output as `output` frames, then its `exited` frame.

use std::time::Duration;

use axum::extract::ws::WebSocket;

use super::send_command;
use crate::dispatch;

/// Relay `child` on `socket`: the `spawned` ack, each output chunk, then the
/// exit code. `on_exit` runs once the child has exited, whether or not the
/// socket is still served.
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
pub(super) async fn stream_child(
    socket: &mut WebSocket,
    id: u64,
    verb: &str,
    mut child: Box<dyn dispatch::Child>,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    on_exit: impl FnOnce() + Send + 'static,
) {
    let pid = child.pid();
    // Take the merged output reader BEFORE `child` moves into the wait task, so
    // the drain and the wait run concurrently (each owns its half).
    let output = child.take_output();
    send_command(
        socket,
        id,
        verb,
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
    // `on_exit` runs HERE, inside the blocking task, and not from the wait arm
    // below: the shutdown and client-disconnect arms `break` without ever
    // polling that arm while the child keeps living (the teardown invariant),
    // and tokio never cancels a blocking task — so this is the only site that
    // fires on every exit path the daemon outlives.
    let mut wait = tokio::task::spawn_blocking(move || {
        let result = child.wait();
        on_exit();
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
                            socket,
                            id,
                            verb,
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
                        socket,
                        id,
                        verb,
                        serde_json::json!({
                            "status": "output",
                            "chunk": String::from_utf8_lossy(&chunk),
                        }),
                    )
                    .await;
                }
                let code = joined.ok().and_then(|r| r.ok()).flatten();
                send_command(
                    socket,
                    id,
                    verb,
                    serde_json::json!({ "status": "exited", "code": code }),
                )
                .await;
                break;
            }
        }
    }
}
