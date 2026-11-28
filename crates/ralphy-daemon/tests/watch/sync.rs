//! Waits on a condition for the watch tests, in place of fixed sleeps. The
//! daemon sends no answer when a watch is armed, so a test acts again while it
//! waits, or asks for an answer that comes only after its earlier frames.

use std::time::Duration;

use ralphy_daemon::protocol::{self, Command, Frame};
use tokio_tungstenite::tungstenite::Message;

use crate::page::Channel as Ws;

/// The time between two runs of the action of [`push_while`]: longer than the
/// 300 ms debounce of the watcher, so each run is seen as its own change.
const ACT_EVERY: Duration = Duration::from_millis(400);

/// The payload of the first `verb` push within `window`, while `act` runs
/// again every 400 ms: a watch that is armed late still sees a change.
/// `None` when no push came or the socket closed.
pub(crate) async fn push_while(
    ws: &mut Ws,
    verb: &str,
    window: Duration,
    mut act: impl FnMut(),
) -> Option<serde_json::Value> {
    let mut tick = tokio::time::interval(ACT_EVERY);
    let wait = async {
        loop {
            tokio::select! {
                _ = tick.tick() => act(),
                msg = ws.next() => {
                    let bytes = match msg? {
                        Ok(Message::Binary(b)) => b,
                        Ok(Message::Close(_)) | Err(_) => return None,
                        _ => continue,
                    };
                    if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                        if cmd.verb == verb {
                            return Some(cmd.payload);
                        }
                    }
                }
            }
        }
    };
    tokio::time::timeout(window, wait).await.ok().flatten()
}

/// Read and drop every `verb` push until one second passes with none, so a
/// push that an earlier action caused is not read as the next one.
pub(crate) async fn drain(ws: &mut Ws, verb: &str) {
    let quiet = Duration::from_secs(1);
    while push_while(ws, verb, quiet, || {}).await.is_some() {}
}

/// Return once the daemon has handled every frame sent before on `ws`. One
/// task reads the frames of a channel in order, so the `tree.failed` answer
/// to a watch of a missing folder comes after them.
pub(crate) async fn barrier(ws: &mut Ws, repo: &str) {
    let frame = Frame::Command(Command {
        id: 0,
        verb: "watch".to_owned(),
        payload: serde_json::json!({ "repo": repo, "path": "no-such-barrier-dir" }),
    });
    ws.send(Message::Binary(protocol::encode(&frame).into()))
        .await
        .expect("send the barrier frame");
    let answer = push_while(ws, "tree.failed", Duration::from_secs(10), || {}).await;
    assert!(
        answer.is_some(),
        "the daemon never answered the barrier watch"
    );
}
