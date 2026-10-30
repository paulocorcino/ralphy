//! The peer relay for `/ws/session`: the query the owning daemon receives
//! and the byte-for-byte bridge between the browser and the peer socket.

use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use axum::http::StatusCode;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;

use super::outbound::CONSOLE_QUEUE;
use super::traffic::{Tab, Traffic};
use super::upgrade::Opened;
use super::SessionQuery;
use crate::mux::{ChannelSender, Mode, Out, Permit, Writer, WriterClosed};
use crate::peer;
use crate::protocol::{self, Frame};
use crate::routes::audit_layer::{ConsoleRecord, SocketAudit};
use crate::routes::encode_query_value;
use crate::session;

/// The `end` of a relayed socket the peer closed: why it closed is in the
/// peer's own log line.
const PEER_CLOSED: &str = "peer-closed";

/// How long the relay waits for the browser to take its Close. Past it the
/// writer is stopped and the socket dropped.
const CLOSE_WINDOW: Duration = Duration::from_secs(5);

/// Whether the relay goes on. `Break(true)`: send the browser a Close first.
type Flow = ControlFlow<bool>;

pub(crate) fn peer_session_query(query: &SessionQuery, slug: &str) -> String {
    if let Some(id) = query.id {
        return reattach_query(query, id, slug);
    }
    // A free console on a peer with no distro (ADR-0067 §7): the peer opens it
    // in its own checkout of `slug`.
    if query.console == Some(1) {
        return console_query(query, slug);
    }
    launch_query(query, slug)
}

/// The query of a reattach to session `id`.
fn reattach_query(query: &SessionQuery, id: u64, slug: &str) -> String {
    let mut out = format!("id={id}&repo={}", encode_query_value(slug));
    if query.takeover == Some(1) {
        out.push_str("&takeover=1");
    }
    if query.watch == Some(1) {
        out.push_str("&watch=1");
    }
    push_holder(&mut out, query);
    push_gzip(&mut out, query);
    push_since(&mut out, query);
    push_link(&mut out, query);
    out
}

/// The query of a free console, with its startup command.
fn console_query(query: &SessionQuery, slug: &str) -> String {
    let mut out = format!("console=1&repo={}", encode_query_value(slug));
    if let Some(command) = query.command.as_deref().map(str::trim) {
        if !command.is_empty() {
            out.push_str("&command=");
            out.push_str(&encode_query_value(command));
        }
    }
    push_record(&mut out, query);
    push_holder(&mut out, query);
    push_gzip(&mut out, query);
    push_link(&mut out, query);
    out
}

/// The query of a new agent launch.
fn launch_query(query: &SessionQuery, slug: &str) -> String {
    let mut out = format!(
        "repo={}&agent={}",
        encode_query_value(slug),
        encode_query_value(query.agent.as_deref().unwrap_or_default())
    );
    // The owning daemon resolves the name against ITS registry; an older peer
    // ignores the key and announces no checkout (the shell tolerates absence).
    if let Some(checkout) = query.checkout.as_deref() {
        out.push_str("&checkout=");
        out.push_str(&encode_query_value(checkout));
    }
    // The owning daemon folds it; an older peer ignores the key and keeps the
    // hex name (ADR-0066 §6).
    if let Some(name) = query.name() {
        out.push_str("&name=");
        out.push_str(&encode_query_value(name));
    }
    push_record(&mut out, query);
    push_holder(&mut out, query);
    push_gzip(&mut out, query);
    push_link(&mut out, query);
    out
}

/// The daemon that hosts the session is the one that compresses the replay,
/// so it must know the page can decode it. An older peer ignores the key and
/// sends the replay raw.
fn push_gzip(out: &mut String, query: &SessionQuery) {
    if query.gzip == Some(1) {
        out.push_str("&gzip=1");
    }
}

/// The owning daemon keeps the session's output, so it is the one that can
/// resume a reattach at the tab's byte count (ADR-0079 D8). An older peer
/// ignores the key and sends the snapshot or the raw replay.
fn push_since(out: &mut String, query: &SessionQuery) {
    if let Some(since) = query.since {
        out.push_str(&format!("&since={since}"));
    }
}

/// The network the browser reported, so the peer's own traffic summary has
/// it too. Only the parsed values go on, so they need no encoding. An older
/// peer ignores the keys.
fn push_link(out: &mut String, query: &SessionQuery) {
    let link = query.link();
    if let Some(kind) = link.effective_type {
        out.push_str("&net=");
        out.push_str(kind.as_str());
    }
    if let Some(rtt) = link.rtt_ms {
        out.push_str(&format!("&net_rtt={rtt}"));
    }
    if let Some(down) = link.downlink_mbps {
        out.push_str(&format!("&net_down={down}"));
    }
}

/// The owning daemon keeps one session per window record (ADR-0050 amendment
/// 2026-10-04), so it must know the record of a launch. An older peer ignores
/// the key. Validated like the holder, so it needs no encoding.
fn push_record(out: &mut String, query: &SessionQuery) {
    if let Some(record) = query.record() {
        out.push_str("&record=");
        out.push_str(record);
    }
}

/// The owning daemon keeps the writer slot, so it is the one that must know
/// the holder (ADR-0051 §9 amendment 2026-09-22) and the attempt of its claim
/// (ADR-0079 D7). An older peer ignores both keys.
fn push_holder(out: &mut String, query: &SessionQuery) {
    if let Some(holder) = query.holder() {
        out.push_str("&holder=");
        out.push_str(holder);
        if let Some(attempt) = query.attempt {
            out.push_str(&format!("&attempt={attempt}"));
        }
    }
}

/// The tab that opens a relayed console socket, and the audit line the
/// opening writes once the peer accepts it: a launch or a take-over, or `None`
/// for a plain reattach or a watch.
pub(crate) struct Opening {
    pub(crate) tab: Tab,
    pub(crate) who: SocketAudit,
    pub(crate) record: Option<ConsoleRecord>,
}

/// The peer to dial, the query to open on it, and who this daemon says it is.
pub(crate) struct PeerDial<'a> {
    pub(crate) peer: &'a peer::PeerDescriptor,
    pub(crate) query: &'a str,
    pub(crate) me: peer::client::SelfRef<'a>,
}

/// Open `dial.query` on the peer that owns the session, for the browser
/// socket to relay to. A refused dial is refused with the peer's diagnosis
/// (`502` on a reattach); an HTTP refusal from the peer keeps its own status
/// and body. A peer that announces its refusal in a frame needs nothing here:
/// the frame is relayed like any other.
pub(crate) async fn dial_peer(dial: PeerDial<'_>, opening: Opening) -> Opened {
    let PeerDial {
        peer,
        query: peer_query,
        me,
    } = dial;
    match peer::client::session(peer, peer_query, me).await {
        Ok(peer_socket) => {
            let Opening { tab, who, record } = opening;
            if let Some(record) = &record {
                who.console(record);
            }
            let traffic = Traffic::peer(
                peer.daemon_id.clone(),
                peer.environment.clone(),
                tab,
                Instant::now(),
            );
            Opened::Relay {
                peer: Box::new(peer_socket),
                traffic,
            }
        }
        Err(peer::client::SocketError::Peer(status)) => {
            Opened::refused(StatusCode::BAD_GATEWAY, status.diagnosis(&peer.environment))
        }
        Err(peer::client::SocketError::Http { status, body }) => Opened::refused(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            body,
        ),
    }
}

/// The traffic summary reads the peer's `session-open` and first terminal
/// frame to tell the replay from live output (see [`Traffic::peer_frame`]).
/// The peer forwards its own pings to the browser and the browser's pongs come
/// back here, so the round trip timed here is the browser's leg only.
///
/// The browser side sends through the socket's writer and is read on its
/// own (ADR-0082 D5): a message from the peer waits in `held` for room in
/// the writer, and the peer is not read while it waits, so a slow browser
/// slows the peer as a blocking send did.
pub(crate) async fn peer_session_ws(
    browser: WebSocket,
    mut peer: peer::client::PeerSocket,
    mut traffic: Traffic,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let (sink, mut from_browser) = browser.split();
    let writer = Writer::start(sink, Mode::Plain);
    let out = writer.channel_with_depth(0, CONSOLE_QUEUE);
    let mut held: Option<Out> = None;
    let mut end = None;
    let close = loop {
        let flow = tokio::select! {
            _ = shutdown.changed() => {
                end = Some(session::EndReason::DaemonShutdown.as_wire());
                ControlFlow::Break(false)
            }
            incoming = from_browser.next() => to_peer(incoming, &mut peer, &mut traffic).await,
            permit = out.reserve(), if held.is_some() => hand_on(permit, &mut held),
            incoming = peer.next(), if held.is_none() => {
                to_browser(incoming, &mut held, &mut traffic, &mut end)
            }
        };
        if let ControlFlow::Break(close) = flow {
            break close;
        }
    };
    if close {
        close_browser(&out, &writer).await;
    }
    writer.abort();
    traffic.log(end, Instant::now());
}

/// Relay one read of the browser socket to the peer. `Break` ends the relay.
async fn to_peer(
    incoming: Option<Result<Message, axum::Error>>,
    peer: &mut peer::client::PeerSocket,
    traffic: &mut Traffic,
) -> Flow {
    let Some(Ok(message)) = incoming else {
        close_peer_session(peer).await;
        return ControlFlow::Break(false);
    };
    match from_browser(message, traffic) {
        Step::Send(outbound) => {
            if peer.send(outbound).await.is_err() {
                return ControlFlow::Break(false);
            }
            ControlFlow::Continue(())
        }
        Step::Skip => ControlFlow::Continue(()),
        Step::Close => {
            close_peer_session(peer).await;
            ControlFlow::Break(false)
        }
    }
}

/// Take one read of the peer socket for the browser into `held`. `Break`
/// ends the relay.
fn to_browser(
    incoming: Option<
        Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>,
    >,
    held: &mut Option<Out>,
    traffic: &mut Traffic,
    end: &mut Option<&'static str>,
) -> Flow {
    let Some(Ok(message)) = incoming else {
        return ControlFlow::Break(true);
    };
    match from_peer(message, traffic, end) {
        Step::Send(outbound) => {
            *held = for_writer(outbound);
            ControlFlow::Continue(())
        }
        Step::Skip => ControlFlow::Continue(()),
        Step::Close => ControlFlow::Break(true),
    }
}

/// The writer's item for a message the relay passes to the browser.
fn for_writer(message: Message) -> Option<Out> {
    match message {
        Message::Binary(bytes) => Some(Out::Frame(Vec::from(bytes))),
        Message::Ping(bytes) => Some(Out::Ping(Vec::from(bytes))),
        Message::Pong(bytes) => Some(Out::Pong(Vec::from(bytes))),
        Message::Close(_) => Some(Out::Close),
        // `from_peer` drops every text frame.
        Message::Text(_) => None,
    }
}

/// Hand the held message to the writer. A writer that has ended means the
/// browser is gone: the relay ends without a Close.
fn hand_on(permit: Result<Permit, WriterClosed>, held: &mut Option<Out>) -> Flow {
    match permit {
        Ok(permit) => {
            if let Some(out) = held.take() {
                permit.put(out);
            }
            ControlFlow::Continue(())
        }
        Err(WriterClosed) => ControlFlow::Break(false),
    }
}

async fn close_browser(out: &ChannelSender, writer: &Writer) {
    let closed = async {
        out.put(Out::Close).await?;
        writer.ended().await;
        Ok(())
    };
    match tokio::time::timeout(CLOSE_WINDOW, closed).await {
        // Closed, or the browser was already gone.
        Ok(Ok(()) | Err(WriterClosed)) => {}
        Err(_) => tracing::debug!("the browser did not take the close in time"),
    }
}

/// What the relay does with one frame it read.
#[derive(Debug)]
enum Step<M> {
    /// Pass this frame on to the other side.
    Send(M),
    /// Drop the frame and read the next one.
    Skip,
    /// The side that sent it closed: close the other side and stop.
    Close,
}

/// One frame from the browser, for the peer. A Close is recorded in
/// `traffic` here, before the relay closes the peer session.
fn from_browser(
    message: Message,
    traffic: &mut Traffic,
) -> Step<tokio_tungstenite::tungstenite::Message> {
    match message {
        Message::Binary(bytes) => {
            traffic.inbound(bytes.len());
            // Read only for the log; the bytes go on unchanged.
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                traffic.client_command(&cmd);
            }
            Step::Send(tokio_tungstenite::tungstenite::Message::Binary(bytes))
        }
        Message::Ping(bytes) => Step::Send(tokio_tungstenite::tungstenite::Message::Ping(bytes)),
        Message::Pong(bytes) => {
            traffic.pong_received(&bytes, Instant::now());
            Step::Send(tokio_tungstenite::tungstenite::Message::Pong(bytes))
        }
        Message::Close(_) => {
            traffic.client_closed();
            Step::Close
        }
        Message::Text(_) => Step::Skip,
    }
}

/// One frame from the peer, for the browser. A peer Close sets `end` before
/// the relay sends the Close on to the browser.
fn from_peer(
    message: tokio_tungstenite::tungstenite::Message,
    traffic: &mut Traffic,
    end: &mut Option<&'static str>,
) -> Step<Message> {
    match message {
        tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
            traffic.peer_frame(&bytes);
            Step::Send(Message::Binary(bytes))
        }
        tokio_tungstenite::tungstenite::Message::Ping(bytes) => {
            traffic.ping_sent(&bytes, Instant::now());
            Step::Send(Message::Ping(bytes))
        }
        tokio_tungstenite::tungstenite::Message::Pong(bytes) => Step::Send(Message::Pong(bytes)),
        tokio_tungstenite::tungstenite::Message::Close(_) => {
            *end = Some(PEER_CLOSED);
            Step::Close
        }
        tokio_tungstenite::tungstenite::Message::Text(_)
        | tokio_tungstenite::tungstenite::Message::Frame(_) => Step::Skip,
    }
}

pub(crate) async fn close_peer_session(peer: &mut peer::client::PeerSocket) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the peer may already be gone"
    )]
    let _ = peer
        .send(tokio_tungstenite::tungstenite::Message::Close(None))
        .await;
    if let tokio_tungstenite::MaybeTlsStream::Plain(stream) = peer.get_mut() {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the peer may already be gone"
        )]
        let _ = stream.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::Message as PeerMessage;

    fn traffic() -> Traffic {
        Traffic::peer("d1".into(), "Linux".into(), Tab::default(), Instant::now())
    }

    #[test]
    fn a_browser_frame_is_counted_and_passed_on_and_a_text_frame_is_dropped() {
        let mut traffic = traffic();
        let step = from_browser(Message::Binary(vec![1, 2, 3].into()), &mut traffic);
        assert!(
            matches!(step, Step::Send(PeerMessage::Binary(ref b)) if b[..] == [1, 2, 3]),
            "{step:?}"
        );
        assert_eq!(traffic.summary().in_bytes, 3);
        let step = from_browser(Message::Text("detach".into()), &mut traffic);
        assert!(matches!(step, Step::Skip), "{step:?}");
        let step = from_browser(Message::Close(None), &mut traffic);
        assert!(matches!(step, Step::Close), "{step:?}");
        assert_eq!(traffic.summary().in_bytes, 3, "only a binary frame counts");
    }

    #[test]
    fn a_peer_close_sets_the_end_and_a_peer_ping_is_timed_by_the_browser_pong() {
        let mut traffic = traffic();
        let mut end = None;
        let output = vec![protocol::TAG_TERMINAL, b'a', b'b'];
        let step = from_peer(PeerMessage::Binary(output.into()), &mut traffic, &mut end);
        assert!(matches!(step, Step::Send(Message::Binary(_))), "{step:?}");
        assert_eq!(traffic.summary().live_bytes, 3);
        let step = from_peer(PeerMessage::Ping(vec![9].into()), &mut traffic, &mut end);
        assert!(matches!(step, Step::Send(Message::Ping(_))), "{step:?}");
        let step = from_browser(Message::Pong(vec![9].into()), &mut traffic);
        assert!(matches!(step, Step::Send(PeerMessage::Pong(_))), "{step:?}");
        assert_eq!(
            traffic.summary().rtt_samples,
            1,
            "the pong answers the ping"
        );
        let step = from_peer(PeerMessage::Text("x".into()), &mut traffic, &mut end);
        assert!(matches!(step, Step::Skip), "{step:?}");
        assert_eq!(end, None);
        let step = from_peer(PeerMessage::Close(None), &mut traffic, &mut end);
        assert!(matches!(step, Step::Close), "{step:?}");
        assert_eq!(end, Some(PEER_CLOSED));
    }

    /// The network the browser reported goes to the peer on every shape, so the
    /// peer's traffic summary has it too. A value no browser gives is absent, and
    /// does not refuse the socket.
    #[test]
    fn peer_session_query_forwards_the_link_the_browser_reported() {
        let reattach = |net: &str, rtt: &str, down: &str| SessionQuery {
            repo: Some("x".into()),
            id: Some(7),
            net: Some(net.into()),
            net_rtt: Some(rtt.into()),
            net_down: Some(down.into()),
            ..SessionQuery::default()
        };
        assert_eq!(
            peer_session_query(&reattach("4g", "100", "1.75"), "owner/repo"),
            "id=7&repo=owner%2Frepo&net=4g&net_rtt=100&net_down=1.75"
        );
        let console = SessionQuery {
            id: None,
            console: Some(1),
            ..reattach("slow-2g", "2000", "0")
        };
        assert_eq!(
            peer_session_query(&console, "owner/repo"),
            "console=1&repo=owner%2Frepo&net=slow-2g&net_rtt=2000&net_down=0"
        );
        let launch = SessionQuery {
            id: None,
            agent: Some("claude".into()),
            ..reattach("2g", "50", "10")
        };
        assert_eq!(
            peer_session_query(&launch, "owner/repo"),
            "repo=owner%2Frepo&agent=claude&net=2g&net_rtt=50&net_down=10"
        );
        assert_eq!(
            peer_session_query(&reattach("3g", "x", "0.5"), "owner/repo"),
            "id=7&repo=owner%2Frepo&net=3g&net_down=0.5",
            "each value is read alone"
        );
        for (net, rtt, down) in [("5g", "-1", "NaN"), ("wifi", "x", "-2"), ("", "1e9", "inf")] {
            assert_eq!(
                peer_session_query(&reattach(net, rtt, down), "owner/repo"),
                "id=7&repo=owner%2Frepo",
                "{net} {rtt} {down}"
            );
        }
    }
}
