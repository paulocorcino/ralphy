//! The peer relay for `/ws/session`: the query the owning daemon receives
//! and the byte-for-byte bridge between the browser and the peer socket.

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;

use super::SessionQuery;
use crate::peer;
use crate::routes::encode_query_value;

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
    push_holder(&mut out, query);
    out
}

/// The owning daemon keeps the writer slot, so it is the one that must know
/// the holder (ADR-0051 §9 amendment 2026-09-22). An older peer ignores the key.
fn push_holder(out: &mut String, query: &SessionQuery) {
    if let Some(holder) = query.holder() {
        out.push_str("&holder=");
        out.push_str(holder);
    }
}

pub(crate) async fn peer_session_ws(
    mut browser: WebSocket,
    mut peer: peer::client::PeerSocket,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            incoming = browser.recv() => {
                let Some(Ok(message)) = incoming else {
                    close_peer_session(&mut peer).await;
                    break;
                };
                let outbound = match message {
                    Message::Binary(bytes) => tokio_tungstenite::tungstenite::Message::Binary(bytes),
                    Message::Ping(bytes) => tokio_tungstenite::tungstenite::Message::Ping(bytes),
                    Message::Pong(bytes) => tokio_tungstenite::tungstenite::Message::Pong(bytes),
                    Message::Close(_) => {
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
                    tokio_tungstenite::tungstenite::Message::Binary(bytes) => Message::Binary(bytes),
                    tokio_tungstenite::tungstenite::Message::Ping(bytes) => Message::Ping(bytes),
                    tokio_tungstenite::tungstenite::Message::Pong(bytes) => Message::Pong(bytes),
                    tokio_tungstenite::tungstenite::Message::Close(_) => {
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
}

pub(crate) async fn close_peer_session(peer: &mut peer::client::PeerSocket) {
    let _ = peer
        .send(tokio_tungstenite::tungstenite::Message::Close(None))
        .await;
    if let tokio_tungstenite::MaybeTlsStream::Plain(stream) = peer.get_mut() {
        let _ = stream.shutdown().await;
    }
}
