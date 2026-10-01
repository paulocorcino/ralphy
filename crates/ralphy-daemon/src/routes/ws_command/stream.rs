//! The streaming tail shared by the Spawn verbs: relay a dispatched child's
//! output as `output` frames, then its `exited` frame.

use std::io::Read;
use std::time::Duration;

use axum::extract::ws::WebSocket;
use tokio::sync::mpsc;

use super::send_command;
use crate::dispatch;

/// Decoded output chunks that may wait for the browser. Each chunk is one read
/// of at most 8 KB, so one run buffers at most about 512 KB; past that the
/// drain waits for the socket.
const OUTPUT_CHANNEL_CAP: usize = 64;

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
    // child never stalls on a full pipe (see dispatch.rs OUTPUT STREAMING). The
    // channel is bounded: a slow browser makes the drain wait, and a dropped
    // receiver makes it discard (see `drain`).
    let (tx, mut rx) = mpsc::channel::<String>(OUTPUT_CHANNEL_CAP);
    if let Some(reader) = output {
        tokio::task::spawn_blocking(move || drain(reader, tx));
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
                                "chunk": chunk,
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
                            "chunk": chunk,
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

/// Read `reader` to EOF and send it on `tx` as text. A send waits while the
/// channel is full; once the receiver is gone the rest is read and discarded,
/// so the child never stalls on a full pipe.
fn drain(mut reader: impl Read, tx: mpsc::Sender<String>) {
    let mut buf = [0u8; 8192];
    let mut carry = Vec::new();
    let mut open = true;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let text = decode_utf8(&mut carry, &buf[..n]);
                if open && !text.is_empty() && tx.blocking_send(text).is_err() {
                    open = false;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                tracing::warn!(error = %e, "reading a dispatched child's output failed");
                break;
            }
        }
    }
    if open && !carry.is_empty() {
        let rest = String::from_utf8_lossy(&carry).into_owned();
        if tx.blocking_send(rest).is_err() {
            tracing::debug!("the output receiver closed before the last bytes");
        }
    }
}

/// Decode `bytes` after the `carry` of the previous read. An incomplete UTF-8
/// sequence at the end stays in `carry` for the next read; any other invalid
/// byte becomes U+FFFD.
fn decode_utf8(carry: &mut Vec<u8>, bytes: &[u8]) -> String {
    carry.extend_from_slice(bytes);
    let split = carry.len() - incomplete_tail(carry);
    let text = String::from_utf8_lossy(&carry[..split]).into_owned();
    carry.drain(..split);
    text
}

/// The length of an incomplete UTF-8 sequence at the end of `b` (0 to 3).
fn incomplete_tail(b: &[u8]) -> usize {
    for back in 1..=b.len().min(3) {
        let byte = b[b.len() - back];
        if byte & 0xC0 == 0x80 {
            continue;
        }
        let need = match byte {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => 1,
        };
        return if need > back { back } else { 0 };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn a_character_split_across_reads_is_not_broken() {
        let mut carry = Vec::new();
        assert_eq!(decode_utf8(&mut carry, &[0xC3]), "");
        assert_eq!(decode_utf8(&mut carry, &[0xA9]), "é");
        assert_eq!(decode_utf8(&mut carry, &[0xE2]), "");
        assert_eq!(decode_utf8(&mut carry, &[0x82, 0xAC]), "€");
        assert!(carry.is_empty());
        assert_eq!(decode_utf8(&mut carry, &[0xFF]), "\u{FFFD}");
        assert_eq!(decode_utf8(&mut carry, b"a"), "a");
    }

    /// Serves `left` reads of `b"x"`, then EOF, and counts every read.
    struct Counting {
        left: usize,
        reads: Arc<AtomicUsize>,
    }

    impl Read for Counting {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.left == 0 {
                return Ok(0);
            }
            self.left -= 1;
            buf[0] = b'x';
            Ok(1)
        }
    }

    #[test]
    fn the_drain_waits_on_a_full_channel_and_reads_to_eof_once_dropped() {
        let reads = Arc::new(AtomicUsize::new(0));
        let reader = Counting {
            left: 1000,
            reads: reads.clone(),
        };
        let (tx, rx) = mpsc::channel(OUTPUT_CHANNEL_CAP);
        let (done_tx, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drain(reader, tx);
            done_tx.send(()).expect("the test waits for the drain");
        });
        std::thread::sleep(Duration::from_millis(300));
        let read = reads.load(Ordering::SeqCst);
        assert!(
            read <= OUTPUT_CHANNEL_CAP + 2,
            "the drain read {read} chunks while nothing received"
        );
        drop(rx);
        done.recv_timeout(Duration::from_secs(2))
            .expect("the drain must read to EOF once the receiver is gone");
        assert_eq!(reads.load(Ordering::SeqCst), 1001);
    }
}
