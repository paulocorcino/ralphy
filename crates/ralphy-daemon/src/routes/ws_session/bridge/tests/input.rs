use super::*;
use futures_util::SinkExt;

/// The rows and columns the client asks for; the console starts at 24×80.
const ASKED: (u16, u16) = (33, 101);

/// A client that reads nothing while 2 MiB of output waits for it still
/// reaches the session with its input. The connection is a 4 KiB pipe, so
/// the output fills it at once and every send to the client waits. The
/// input is a resize: it reaches the session when the session's size
/// changes, which needs no shell to answer.
#[tokio::test]
async fn input_is_not_held_behind_output() {
    let manager = Arc::new(session::SessionManager::new());
    let spec = session::console_spec(std::env::temp_dir(), 24, 80, None);
    let (id, mut attach) = manager
        .spawn_attached(
            "~".to_string(),
            "console".to_string(),
            "console".to_string(),
            None,
            None,
            None,
            spec,
        )
        .expect("the platform shell must spawn — the free console depends on it");
    // 2 MiB in 4 KiB chunks, the size a PTY read gives, queued for the client.
    let (tx, rx) = tokio::sync::broadcast::channel::<Vec<u8>>(1024);
    attach.rx = rx;
    attach.snapshot.clear();
    for n in 0..512u32 {
        tx.send(vec![b'a' + (n % 26) as u8; 4096])
            .expect("the attachment holds a receiver");
    }
    let mut size = manager
        .watch(id, None)
        .expect("a live session is watchable")
        .size;
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = serve_bridge(OneConnection(Some(server_io)), id, attach, false);
    let (mut client, _) = tokio_tungstenite::client_async("ws://localhost/", client_io)
        .await
        .expect("the bridge upgrades the connection");
    // Give the bridge time to fill the pipe with output before the input.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let resize = Frame::Command(Command {
        id: 0,
        verb: "resize".to_string(),
        payload: serde_json::json!({ "rows": ASKED.0, "cols": ASKED.1 }),
    });
    client
        .send(tungstenite::Message::Binary(
            protocol::encode(&resize).into(),
        ))
        .await
        .expect("the client's own sends do not wait for its reads");
    let reached = tokio::time::timeout(
        Duration::from_millis(500),
        size.wait_for(|now| *now == ASKED),
    )
    .await;
    server.abort();
    manager.close(id);
    assert!(
        matches!(reached, Ok(Ok(_))),
        "the input did not reach the session within 500 ms"
    );
    drop(tx);
}
