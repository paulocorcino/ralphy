//! The `spawn` kind: one command, from the open payload, served as
//! `/ws/command` serves its first frame.
//!
//! The open carries the command as `command: {id, verb, payload}`. It is a
//! bare frame on channel 0, so the command must fit the cap of one. The
//! channel gets the frames of the command socket (`spawned`, `output`,
//! `exited`, or one reply or error), and then ends with `done`. The verb
//! registry, the refusals and the audit line are those of the command
//! socket: this kind calls the same code.
//!
//! The channel never ends a run. The page closing the channel, the socket
//! closing and the daemon stopping all stop only this task, and the run
//! keeps its own life (the teardown invariant of the command socket).

use tokio::sync::broadcast;

use super::{Channel, CloseReason, Opening};
use crate::protocol::Command;
use crate::routes::{serve_command, CommandCtx, SocketAudit, Wire};

/// Serve a `spawn` channel: the command of the open payload. An open with no
/// command is refused.
pub(super) fn open(
    opening: Opening,
    command: CommandCtx,
    who: SocketAudit,
    run_exits: broadcast::Sender<String>,
    bound_port: u16,
) -> Result<Channel, CloseReason> {
    let cmd = command_of(&opening.payload).ok_or(CloseReason::Refused)?;
    Ok(Channel::spawn(move |frames| async move {
        let mut wire = Wire::on_channel(opening.out.clone(), frames);
        serve_command(&mut wire, cmd, command, run_exits, bound_port, who).await;
        drop(wire);
        end(opening).await;
    }))
}

/// The command an open names.
fn command_of(payload: &serde_json::Value) -> Option<Command> {
    serde_json::from_value(payload.get("command")?.clone()).ok()
}

/// End the channel with `done`, after the last frame of the run is sent.
async fn end(opening: Opening) {
    // `channel.closed` goes on channel 0, which the writer may send before
    // the last frame of this channel.
    if opening.out.sent().await.is_err() {
        // The socket is gone, and the page with it.
        return;
    }
    opening.end(CloseReason::Done).await;
}
