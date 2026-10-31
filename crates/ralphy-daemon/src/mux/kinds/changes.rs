//! The `changes` kind: each run exit the daemon hears goes to the page as
//! `changes.dirty {repo}`. The page filters by the repo it has open.

use tokio::sync::{broadcast, mpsc};

use super::{Channel, CloseReason, Opening};
use crate::mux::ChannelSender;
use crate::routes::relay_run_exit;

/// Serve a `changes` channel from the run exits `run_exits` hears.
pub(super) fn open(
    opening: Opening,
    run_exits: broadcast::Receiver<String>,
) -> Result<Channel, CloseReason> {
    let out = opening.out;
    Ok(Channel::spawn(move |frames| serve(frames, run_exits, out)))
}

async fn serve(
    mut frames: mpsc::Receiver<Vec<u8>>,
    mut run_exits: broadcast::Receiver<String>,
    out: ChannelSender,
) {
    loop {
        tokio::select! {
            // A changes channel reads nothing the page sends. The frames are
            // taken so the socket's reader never waits on this channel.
            frame = frames.recv() => if frame.is_none() {
                break;
            },
            exited = run_exits.recv() => {
                if !relay_run_exit(&out, exited).await {
                    break;
                }
            }
        }
    }
}
