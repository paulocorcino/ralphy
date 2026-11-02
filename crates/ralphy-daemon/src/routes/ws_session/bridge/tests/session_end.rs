use super::*;

/// A child that exits leaves its last output queued on the attachment
/// while the eviction is already signalled. Every queued chunk reaches the
/// client before `session-end`. With both arms ready, an unbiased select
/// takes the eviction about half the time, so a loop that drops queued
/// output keeps all 32 chunks with a chance near 2^-32.
#[tokio::test]
async fn queued_output_reaches_the_client_before_the_session_end() {
    let chunks: Vec<Vec<u8>> = (0..32)
        .map(|n| format!("line {n}\r\n").into_bytes())
        .collect();
    let (id, attach) = exited_session(&chunks, 64);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let addr = listener
        .local_addr()
        .expect("a bound listener has an address");
    let server = serve_bridge(listener, id, attach, false);

    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
        .await
        .expect("the bridge accepts the socket");
    let seen = tokio::time::timeout(
        Duration::from_secs(10),
        read_until_end(&mut client, Duration::ZERO),
    )
    .await
    .expect("the bridge sends session-end");
    server.abort();

    assert_eq!(
        String::from_utf8_lossy(&seen.received),
        String::from_utf8_lossy(&chunks.concat()),
        "every queued chunk reaches the client"
    );
    assert!(seen.ended_after.is_some(), "the end is announced");
}

/// A client that takes the queue too slowly cannot hold back the end:
/// the drain stops at its own limit, so `session-end` is still sent
/// inside the 5 s end window. The connection is a 4 KiB pipe
/// and the client reads one chunk per 100 ms, so 100 chunks need about
/// 10 s. Time is paused: tokio moves the clock forward only while every
/// task waits, so the limits are measured in test time, not in the speed
/// of the machine.
#[tokio::test(start_paused = true)]
async fn a_drain_that_cannot_finish_still_sends_the_end() {
    let chunks: Vec<Vec<u8>> = (0..100u8).map(|n| vec![n; 64 * 1024]).collect();
    let (id, attach) = exited_session(&chunks, 128);
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = serve_bridge(OneConnection(Some(server_io)), id, attach, false);

    let (mut client, _) = tokio_tungstenite::client_async("ws://localhost/", client_io)
        .await
        .expect("the bridge upgrades the connection");
    let seen = tokio::time::timeout(
        Duration::from_secs(15),
        read_until_end(&mut client, Duration::from_millis(100)),
    )
    .await
    .expect("the bridge sends session-end inside its end window");
    server.abort();

    assert!(seen.ended_after.is_some(), "the end is announced");
    assert!(
        seen.received.len() < chunks.concat().len(),
        "the drain stopped at its limit instead of sending everything"
    );
}
