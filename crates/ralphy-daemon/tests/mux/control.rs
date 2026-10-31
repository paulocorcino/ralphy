//! Channel 0 of `/ws`: `channel.open`, `channel.close` and the
//! `channel.closed` the daemon answers, and the limits of a bare frame.

use std::time::Duration;

use futures_util::SinkExt;
use ralphy_daemon::protocol::{self, Frame};
use tokio_tungstenite::tungstenite::Message;

use crate::support::{
    close, connect, ends_within, next_closed, next_inner, open, send_control, send_on, serve, Ws,
};

const WITHIN: Duration = Duration::from_secs(5);

/// The `channel.closed` the next frames carry for `channel`, as its reason.
async fn closed_reason(ws: &mut Ws, channel: u64) -> String {
    let closed = next_closed(ws, WITHIN)
        .await
        .expect("a channel.closed within 5 s");
    assert_eq!(
        closed.channel, channel,
        "the channel.closed names the channel"
    );
    closed.reason
}

/// A presence frame on channel 0 within 5 s: the socket is still open.
async fn assert_still_open(ws: &mut Ws) {
    let deadline = tokio::time::Instant::now() + WITHIN;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let inner = next_inner(ws, 0, left)
            .await
            .expect("the socket stays open and sends its heartbeat");
        if let Ok(Frame::Presence(_)) = protocol::decode(&inner) {
            return;
        }
    }
}

#[tokio::test]
async fn a_duplicate_channel_id_is_refused() {
    let daemon = serve().await;
    let mut ws = connect(&daemon).await;
    open(&mut ws, 1, "tree", serde_json::json!({})).await;
    // The first open is answered by its kind; only the second one is the
    // duplicate this test is about.
    let first = next_closed(&mut ws, Duration::from_millis(500)).await;
    open(&mut ws, 1, "tree", serde_json::json!({})).await;
    assert_eq!(closed_reason(&mut ws, 1).await, "refused");
    if first.is_none() {
        // The first open is still open: the duplicate did not close it.
        assert!(next_closed(&mut ws, Duration::from_millis(500))
            .await
            .is_none());
    }
}

#[tokio::test]
async fn a_closed_channel_id_is_never_reused() {
    let daemon = serve().await;
    let mut ws = connect(&daemon).await;
    open(&mut ws, 3, "tree", serde_json::json!({})).await;
    close(&mut ws, 3).await;
    // Whatever the first open got, the page closed the id.
    let _first = next_closed(&mut ws, Duration::from_millis(500)).await;
    open(&mut ws, 3, "tree", serde_json::json!({})).await;
    assert_eq!(closed_reason(&mut ws, 3).await, "refused");
}

/// `spawn` with no command: a kind that cannot use the open refuses it, and
/// the refusal ends only that channel.
#[tokio::test]
async fn an_open_of_a_refused_kind_keeps_the_socket() {
    let daemon = serve().await;
    let mut ws = connect(&daemon).await;
    open(&mut ws, 7, "spawn", serde_json::json!({})).await;
    assert_eq!(closed_reason(&mut ws, 7).await, "refused");
    assert_still_open(&mut ws).await;
}

/// A piece on an id the page never opened is refused, and the socket stays
/// open.
#[tokio::test]
async fn a_piece_on_an_unknown_channel_is_refused() {
    let daemon = serve().await;
    let mut ws = connect(&daemon).await;
    send_on(&mut ws, 9, b"\x02{}").await;
    assert_eq!(closed_reason(&mut ws, 9).await, "refused");
    assert_still_open(&mut ws).await;
}

/// An open of a kind the daemon does not know is refused.
#[tokio::test]
async fn an_open_of_an_unknown_kind_is_refused() {
    let daemon = serve().await;
    let mut ws = connect(&daemon).await;
    send_control(
        &mut ws,
        "channel.open",
        serde_json::json!({ "channel": 4, "kind": "nothing" }),
    )
    .await;
    assert_eq!(closed_reason(&mut ws, 4).await, "refused");
}

/// A bare frame is a presence report or a control verb, both small: one past
/// the report cap ends the socket, as the cap of the socket did before it
/// took channel frames. A piece of the same size is within its own cap.
#[tokio::test]
async fn a_bare_frame_past_the_report_cap_closes_the_socket() {
    let daemon = serve().await;
    let big = serde_json::json!({ "pad": "x".repeat(1024) });

    let mut ws = connect(&daemon).await;
    send_control(&mut ws, "presence.report", big.clone()).await;
    assert!(
        ends_within(&mut ws, WITHIN).await,
        "a bare frame of 1 KiB ends the socket"
    );

    // Negative control: the same bytes as a piece on a channel do not end
    // the socket.
    let mut ws = connect(&daemon).await;
    let bytes = protocol::encode(&Frame::Command(protocol::Command {
        id: 0,
        verb: "presence.report".to_string(),
        payload: big,
    }));
    send_on(&mut ws, 2, &bytes).await;
    assert_eq!(closed_reason(&mut ws, 2).await, "refused");
    assert_still_open(&mut ws).await;
    ws.send(Message::Close(None))
        .await
        .expect("close the page socket");
}
