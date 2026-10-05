//! The peer relay for `/ws/session`: the query the owning daemon receives
//! and the byte-for-byte bridge between the browser and the peer socket.

use std::time::Instant;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;

use super::refuse::Refuser;
use super::traffic::{Leave, Traffic};
use super::SessionQuery;
use crate::peer;
use crate::protocol::{self, Frame};
use crate::routes::encode_query_value;
use crate::session;

/// The `end` of a relayed socket the peer closed: why it closed is in the
/// peer's own log line.
const PEER_CLOSED: &str = "peer-closed";

pub(crate) fn peer_session_query(query: &SessionQuery, slug: &str) -> String {
    if let Some(id) = query.id {
        let mut out = format!("id={id}&repo={}", encode_query_value(slug));
        if query.takeover == Some(1) {
            out.push_str("&takeover=1");
        }
        if query.watch == Some(1) {
            out.push_str("&watch=1");
        }
        push_holder(&mut out, query);
        return out;
    }
    // A free console on a peer with no distro (ADR-0067 §7): the peer opens it
    // in its own checkout of `slug`.
    if query.console == Some(1) {
        let mut out = format!("console=1&repo={}", encode_query_value(slug));
        if let Some(command) = query.command.as_deref().map(str::trim) {
            if !command.is_empty() {
                out.push_str("&command=");
                out.push_str(&encode_query_value(command));
            }
        }
        push_record(&mut out, query);
        push_holder(&mut out, query);
        return out;
    }
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
    out
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
/// the holder (ADR-0051 §9 amendment 2026-09-22). An older peer ignores the key.
fn push_holder(out: &mut String, query: &SessionQuery) {
    if let Some(holder) = query.holder() {
        out.push_str("&holder=");
        out.push_str(holder);
    }
}

/// Open `peer_query` on the peer that owns the session and bridge it to the
/// browser. A refused dial is refused with the peer's diagnosis (`502` on a
/// reattach); an HTTP refusal from the peer keeps its own status and body. A
/// peer that announces its refusal in a frame needs nothing here: the frame is
/// relayed like any other.
pub(crate) async fn relay_to_peer(
    ws: WebSocketUpgrade,
    peer: &peer::PeerDescriptor,
    peer_query: &str,
    holder: Option<String>,
    me: peer::client::SelfRef<'_>,
    refuser: &Refuser,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Response {
    match peer::client::session(peer, peer_query, me).await {
        Ok(peer_socket) => {
            let traffic = Traffic::peer(
                peer.daemon_id.clone(),
                peer.environment.clone(),
                holder,
                Instant::now(),
            );
            ws.on_upgrade(move |socket| peer_session_ws(socket, peer_socket, traffic, shutdown))
        }
        Err(peer::client::SocketError::Peer(status)) => refuser.refuse(
            ws,
            StatusCode::BAD_GATEWAY,
            status.diagnosis(&peer.environment),
        ),
        Err(peer::client::SocketError::Http { status, body }) => refuser.refuse(
            ws,
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            body,
        ),
    }
}

/// The relay cannot tell a replay from live output without decoding the
/// frames, so its traffic summary counts every byte from the peer as live.
/// The peer forwards its own pings to the browser and the browser's pongs come
/// back here, so the round trip timed here is the browser's leg only.
pub(crate) async fn peer_session_ws(
    mut browser: WebSocket,
    mut peer: peer::client::PeerSocket,
    mut traffic: Traffic,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut end = None;
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                end = Some(session::EndReason::DaemonShutdown.as_wire());
                break;
            }
            incoming = browser.recv() => {
                let Some(Ok(message)) = incoming else {
                    close_peer_session(&mut peer).await;
                    break;
                };
                let outbound = match message {
                    Message::Binary(bytes) => {
                        traffic.inbound(bytes.len());
                        // Read only for the log; the bytes go on unchanged.
                        if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                            if let Some(leave) = Leave::from_command(&cmd) {
                                traffic.client_left(leave);
                            }
                        }
                        tokio_tungstenite::tungstenite::Message::Binary(bytes)
                    }
                    Message::Ping(bytes) => tokio_tungstenite::tungstenite::Message::Ping(bytes),
                    Message::Pong(bytes) => {
                        traffic.pong_received(&bytes, Instant::now());
                        tokio_tungstenite::tungstenite::Message::Pong(bytes)
                    }
                    Message::Close(_) => {
                        traffic.client_closed();
                        close_peer_session(&mut peer).await;
                        break;
                    }
                    Message::Text(_) => continue,
                };
                if peer.send(outbound).await.is_err() {
                    break;
                }
            }
            incoming = peer.next() => {
                let Some(Ok(message)) = incoming else {
                    let _ = browser.send(Message::Close(None)).await;
                    break;
                };
                let outbound = match message {
                    tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
                        traffic.live(bytes.len());
                        Message::Binary(bytes)
                    }
                    tokio_tungstenite::tungstenite::Message::Ping(bytes) => {
                        traffic.ping_sent(&bytes, Instant::now());
                        Message::Ping(bytes)
                    }
                    tokio_tungstenite::tungstenite::Message::Pong(bytes) => Message::Pong(bytes),
                    tokio_tungstenite::tungstenite::Message::Close(_) => {
                        end = Some(PEER_CLOSED);
                        let _ = browser.send(Message::Close(None)).await;
                        break;
                    }
                    tokio_tungstenite::tungstenite::Message::Text(_)
                    | tokio_tungstenite::tungstenite::Message::Frame(_) => continue,
                };
                if browser.send(outbound).await.is_err() {
                    break;
                }
            }
        }
    }
    traffic.log(end, Instant::now());
}

pub(crate) async fn close_peer_session(peer: &mut peer::client::PeerSocket) {
    let _ = peer
        .send(tokio_tungstenite::tungstenite::Message::Close(None))
        .await;
    if let tokio_tungstenite::MaybeTlsStream::Plain(stream) = peer.get_mut() {
        let _ = stream.shutdown().await;
    }
}
