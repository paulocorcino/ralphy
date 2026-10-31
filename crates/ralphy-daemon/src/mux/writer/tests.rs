use super::*;
use crate::protocol::{Frame, Joiner};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite;

type Client = tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>;

/// One in-memory connection, accepted once (copied from the bridge tests).
struct OneConnection(Option<tokio::io::DuplexStream>);

impl axum::serve::Listener for OneConnection {
    type Io = tokio::io::DuplexStream;
    type Addr = ();

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.0.take() {
            Some(io) => (io, ()),
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(())
    }
}

/// A real WebSocket over an in-memory pipe: the server side splits the
/// socket, drops its read half and starts the writer on the sink half.
/// Returns the client and the writer.
async fn connect() -> (Client, Writer) {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let (give, take) = tokio::sync::oneshot::channel::<Writer>();
    let give = Arc::new(Mutex::new(Some(give)));
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(move |ws: WebSocketUpgrade| {
            let give = give
                .lock()
                .expect("give mutex")
                .take()
                .expect("one client connects");
            async move {
                ws.on_upgrade(move |socket| async move {
                    let (sink, _stream) = socket.split();
                    let writer = Writer::start(sink);
                    give.send(writer)
                        .unwrap_or_else(|_| panic!("the test waits for the writer"));
                })
            }
        }),
    );
    tokio::spawn(async move { axum::serve(OneConnection(Some(server_io)), app).await });
    let (client, _) = tokio_tungstenite::client_async("ws://localhost/", client_io)
        .await
        .expect("the server upgrades the connection");
    let writer = take.await.expect("the server starts the writer");
    (client, writer)
}

/// The next Binary message the client gets, or `None` when the socket ends.
async fn next_binary(client: &mut Client) -> Option<Vec<u8>> {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), client.next())
            .await
            .expect("a message or the end within 10 s");
        match message {
            Some(Ok(tungstenite::Message::Binary(bytes))) => return Some(bytes.to_vec()),
            Some(Ok(tungstenite::Message::Close(_)) | Err(_)) | None => return None,
            Some(Ok(
                tungstenite::Message::Text(_)
                | tungstenite::Message::Ping(_)
                | tungstenite::Message::Pong(_)
                | tungstenite::Message::Frame(_),
            )) => {}
        }
    }
}

/// The next channel frame the client gets.
async fn next_piece(client: &mut Client) -> (u32, bool, Vec<u8>) {
    let bytes = next_binary(client).await.expect("the socket is open");
    match protocol::decode(&bytes).expect("a well-formed frame") {
        Frame::Channel {
            channel,
            more,
            piece,
        } => (channel, more, piece),
        other @ (Frame::Terminal { .. } | Frame::Command(_) | Frame::Presence(_)) => {
            panic!("expected a channel frame, got {other:?}")
        }
    }
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8 ^ seed).collect()
}

/// A 1 MiB frame is 64 pieces. When a 10-byte frame of another channel is
/// queued right after it, the writer sends at most one piece of the large
/// frame before the small one.
#[tokio::test]
async fn a_flooding_channel_delays_another_by_one_piece() {
    let (mut client, writer) = connect().await;
    let flood = writer.channel(1);
    let other = writer.channel(2);
    flood
        .send(pattern(1024 * 1024, 0))
        .await
        .expect("the writer is open");
    other.send(vec![7; 10]).await.expect("the writer is open");

    let mut flood_pieces = 0;
    loop {
        let (channel, more, piece) = next_piece(&mut client).await;
        if channel == 2 {
            assert!(!more, "a 10-byte frame is one piece");
            assert_eq!(piece, vec![7; 10]);
            break;
        }
        assert_eq!(channel, 1, "only channels 1 and 2 send");
        flood_pieces += 1;
    }
    assert!(
        flood_pieces <= 1,
        "{flood_pieces} pieces of the flood left before the other channel's frame"
    );
}

/// Two frames of one channel, the first cut into many pieces, with a third
/// channel sending between its pieces: the client joins each channel's
/// pieces in arrival order and gets the frames exactly as they were sent.
#[tokio::test]
async fn the_pieces_of_one_frame_join_to_the_same_bytes() {
    let (mut client, writer) = connect().await;
    let main = writer.channel(5);
    let side = writer.channel(9);
    let first = pattern(5 * protocol::MAX_PIECE + 7, 0x11);
    let second = pattern(2 * protocol::MAX_PIECE, 0x22);
    let side_frames: Vec<Vec<u8>> = (0..4u8).map(|n| vec![n; 100]).collect();
    main.send(first.clone()).await.expect("the writer is open");
    main.send(second.clone()).await.expect("the writer is open");
    for frame in &side_frames {
        side.send(frame.clone()).await.expect("the writer is open");
    }

    let mut joiners = [Joiner::default(), Joiner::default()];
    let mut joined: [Vec<Vec<u8>>; 2] = [Vec::new(), Vec::new()];
    while joined[0].len() < 2 || joined[1].len() < side_frames.len() {
        let (channel, more, piece) = next_piece(&mut client).await;
        let slot = match channel {
            5 => 0,
            9 => 1,
            other => panic!("unexpected channel {other}"),
        };
        assert!(
            piece.len() <= protocol::MAX_PIECE,
            "a piece is at most 16 KiB"
        );
        if let Some(frame) = joiners[slot].push(more, &piece).expect("within the cap") {
            joined[slot].push(frame);
        }
    }
    assert!(
        joined[0] == [first, second],
        "channel 5 joins to its frames"
    );
    assert_eq!(joined[1], side_frames, "channel 9 joins to its frames");
}

/// Frames queued on a channel whose sender is then dropped are all sent.
/// The queue is then freed: with the writer handle dropped too, the writer
/// has nothing left and ends, so the socket ends after the third frame.
#[tokio::test]
async fn a_dropped_channel_drains_then_frees_its_queue() {
    let (mut client, writer) = connect().await;
    let channel = writer.channel(1);
    let frames: Vec<Vec<u8>> = (0..3u8).map(|n| vec![n; 32]).collect();
    for frame in &frames {
        channel
            .send(frame.clone())
            .await
            .expect("the writer is open");
    }
    drop(channel);
    drop(writer);

    for frame in &frames {
        let (channel, more, piece) = next_piece(&mut client).await;
        assert_eq!(channel, 1, "only channel 1 sends");
        assert!(!more, "a 32-byte frame is one piece");
        assert_eq!(
            &piece, frame,
            "a queued frame is sent after its sender is dropped"
        );
    }
    assert_eq!(
        next_binary(&mut client).await,
        None,
        "the writer ends once the dropped channel's queue is freed"
    );
}

/// When the socket is gone, every sender reports the writer closed.
#[tokio::test]
async fn a_closed_socket_makes_every_sender_report_closed() {
    let (client, writer) = connect().await;
    let channel = writer.channel(1);
    drop(client);
    let mut result = Ok(());
    for _ in 0..1000 {
        result = channel.send(pattern(protocol::MAX_PIECE, 1)).await;
        if result.is_err() {
            break;
        }
    }
    assert_eq!(result, Err(WriterClosed));
    assert_eq!(writer.channel(2).send(vec![1]).await, Err(WriterClosed));
}

/// The next message the client gets, of any kind.
async fn next_message(client: &mut Client) -> tungstenite::Message {
    tokio::time::timeout(Duration::from_secs(10), client.next())
        .await
        .expect("a message within 10 s")
        .expect("the socket is open")
        .expect("a well-formed message")
}

/// A Ping queued after a frame leaves after it, whole.
#[tokio::test]
async fn a_ping_follows_the_frame_queued_before_it() {
    let (mut client, writer) = connect().await;
    let channel = writer.channel(0);
    channel
        .send(vec![1, 2, 3])
        .await
        .expect("the writer is open");
    channel
        .put(Out::Ping(vec![9]))
        .await
        .expect("the writer is open");

    let frame = next_message(&mut client).await;
    assert!(
        matches!(&frame, tungstenite::Message::Binary(b) if b[..] == [1, 2, 3]),
        "{frame:?}"
    );
    let ping = next_message(&mut client).await;
    assert!(
        matches!(&ping, tungstenite::Message::Ping(b) if b[..] == [9]),
        "{ping:?}"
    );
}

/// A channel opened with depth 1 makes its sender wait once one item waits
/// behind the one being sent. Aborting the writer of a client that reads
/// nothing ends it at once: the waiting sender then reports closed.
#[tokio::test]
async fn a_shallow_channel_waits_and_an_aborted_writer_ends() {
    let (_client, writer) = connect().await;
    let channel = writer.channel_with_depth(1, 1);
    // The first frame fills the 64 KiB pipe and holds the writer; the
    // second takes the one place in the queue.
    for _ in 0..2 {
        channel
            .send(pattern(1024 * 1024, 3))
            .await
            .expect("the writer is open");
    }
    let third = tokio::time::timeout(Duration::from_millis(200), channel.send(vec![1])).await;
    assert!(third.is_err(), "a third frame waits for room");

    writer.abort();
    let after = tokio::time::timeout(Duration::from_secs(10), channel.send(vec![1]))
        .await
        .expect("an aborted writer ends");
    assert_eq!(after, Err(WriterClosed));
}

#[test]
fn writer_closed_keeps_its_text() {
    assert_eq!(WriterClosed.to_string(), "the socket writer is closed");
}
