//! The output pump of one session: the task that drains the child's bytes and
//! ends the session when the child exits.

use std::sync::{Arc, Weak};
use std::time::SystemTime;

use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedReceiver;

use super::kept::publish;
use super::{evict_all, notify, ManagedSession, SessionManager};
use crate::session::EndReason;

/// The output pump for one session: drain the child's output, keep it (the ring
/// and the screen) AND send it to the live broadcast — both in [`publish`], which
/// holds the EXACTLY-ONCE REPLAY INVARIANT — and remove the session on child EOF.
/// Spawned on the tokio runtime; the `Weak` avoids a manager↔pump reference
/// cycle.
pub(super) fn start_pump(
    sess: Arc<ManagedSession>,
    manager: Weak<SessionManager>,
    mut output: UnboundedReceiver<Vec<u8>>,
    changes: broadcast::Sender<()>,
) {
    tokio::spawn(async move {
        // Poll for a self-exited child alongside draining output: ConPTY does not
        // EOF the reader on child death (only on master drop), so without this a
        // child that exits on its own (e.g. `quit`) would leak its session forever
        // on Windows. On exit, `close()` drops the master → the reader EOFs → the
        // loop below ends. On Unix the reader EOFs directly and the tick is moot.
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                chunk = output.recv() => match chunk {
                    Some(chunk) => publish(&sess.kept, &sess.tx, chunk),
                    None => break, // reader EOF (child exited + master dropped)
                },
                _ = tick.tick() => on_tick(&sess, &changes),
            }
        }
        retire(&sess, &manager, &changes);
    });
}

/// One tick of the pump: fold what the hooks appended, and close a child that
/// exited on its own.
fn on_tick(sess: &ManagedSession, changes: &broadcast::Sender<()>) {
    // The agent-state tail rides the same tick (ADR-0059 §5).
    if sess.poll_status(SystemTime::now()) {
        notify(changes);
    }
    // One lock spanning the check + close so a client write/resize
    // cannot interleave between them.
    let mut session = sess.session.lock().expect("session mutex");
    if session.has_exited() {
        // Drops the master → the reader drains then EOFs → break.
        session.close();
    }
}

/// The end of a session whose child reached EOF: its hook files go, it leaves
/// the list, and every client attached to it is told.
fn retire(sess: &ManagedSession, manager: &Weak<SessionManager>, changes: &broadcast::Sender<()>) {
    // The hooks' files go with the session (§6: never persisted).
    if let Some(status) = &sess.status {
        status.files.remove();
    }
    // Child EOF: a session ends besides `close` only when its child exits
    // (issue #166). Remove it from the list, then evict any attached client so
    // its bridge loop ends and the browser sees the session close.
    match manager.upgrade() {
        // Removal and eviction under ONE `sessions` guard — see `attach`'s
        // REGISTRATION INVARIANT.
        Some(manager) => {
            let mut map = manager.sessions.lock().expect("sessions mutex");
            let removed = map.remove(&sess.info.id).is_some();
            evict_all(sess, EndReason::ChildExited);
            drop(map);
            if removed {
                notify(changes);
            }
        }
        // The manager is gone, so no new attachment can be registered; the
        // ones already holding this session still have to be told.
        None => evict_all(sess, EndReason::ChildExited),
    }
}

#[cfg(test)]
mod tests;
