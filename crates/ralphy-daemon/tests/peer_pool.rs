//! The peer transport reuses its connection (ADR-0052 §2).
//!
//! Not a micro-optimisation: a dial per request cost the DIALLING host one
//! ephemeral port per request, held in `TIME_WAIT` for four minutes. A live
//! workbench against a WSL peer drained a Windows host's 16k-port pool against
//! that single peer, after which every `connect` failed with "address already in
//! use" and the file tree went blank while the peer answered in 3 ms
//! (2026-09-01). This pins the property that makes that unreachable.
//!
//! The peer here is a hand-rolled HTTP/1.1 responder rather than a daemon,
//! because the subject is the SOCKET: it counts accepts, which no daemon-level
//! assertion can see.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ralphy_daemon::peer::{client, PeerDescriptor, PEER_PROTOCOL_VERSION};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn descriptor(port: u16) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: "01TESTPEERPOOLTESTPEERPO".to_string(),
        name: "peer".to_string(),
        avatar: "🐙".to_string(),
        address: "127.0.0.1".to_string(),
        port,
        environment: "WSL: Ubuntu".to_string(),
        token: "peer-tok".to_string(),
        protocol_version: PEER_PROTOCOL_VERSION,
        nudge: None,
    }
}

/// A keep-alive HTTP/1.1 peer that counts the connections it accepts. One read
/// per request is exact here: the client awaits each response before sending the
/// next, so requests can never arrive pipelined into one read.
async fn counting_peer() -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepts = Arc::new(AtomicUsize::new(0));
    let counter = accepts.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0_u8; 8192];
                loop {
                    let read = match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    // `/close-me` answers and then hangs up — a peer restarting
                    // under an idle pooled connection, which the caller has to
                    // survive rather than inherit.
                    let hang_up = String::from_utf8_lossy(&buf[..read]).contains("/close-me");
                    let body = br#"{"status":"ok"}"#;
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: {}\r\n\r\n",
                        body.len(),
                        if hang_up { "close" } else { "keep-alive" }
                    );
                    if sock.write_all(head.as_bytes()).await.is_err()
                        || sock.write_all(body).await.is_err()
                        || hang_up
                    {
                        return;
                    }
                }
            });
        }
    });
    (port, accepts)
}

#[tokio::test]
async fn sequential_peer_requests_share_one_connection() {
    let (port, accepts) = counting_peer().await;
    let peer = descriptor(port);

    for _ in 0..5 {
        let (status, body) = client::get(&peer, "/api/peer/hello").await.unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, br#"{"status":"ok"}"#);
    }
    for _ in 0..5 {
        let (status, _) = client::post_json(&peer, "/api/peer/tree/poll", &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(status, 200);
    }

    assert_eq!(
        accepts.load(Ordering::SeqCst),
        1,
        "ten peer requests must ride one connection, not open ten sockets"
    );
}

/// The pool must not turn a peer that went away into a hung caller: a connection
/// the peer has closed is discarded and redialled, and the request still lands.
#[tokio::test]
async fn a_closed_pooled_connection_is_redialled() {
    let (port, accepts) = counting_peer().await;
    let peer = descriptor(port);

    let (status, _) = client::get(&peer, "/api/peer/hello").await.unwrap();
    assert_eq!(status, 200);
    assert_eq!(accepts.load(Ordering::SeqCst), 1);

    // Ask the peer to hang up on the idle connection the way a restart would.
    let (status, _) = client::get(&peer, "/close-me").await.unwrap();
    assert_eq!(status, 200);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = client::get(&peer, "/api/peer/hello").await.unwrap();
    assert_eq!(
        status, 200,
        "a dead pooled connection must not fail the call"
    );
    assert_eq!(
        accepts.load(Ordering::SeqCst),
        2,
        "the hung-up connection must be redialled, not reused"
    );
}
