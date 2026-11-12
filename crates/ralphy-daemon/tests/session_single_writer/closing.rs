use super::*;

/// The SILENT half of the announcement invariant (issue #334): a bridge that
/// falls out of its loop for any reason OTHER than a deliberate end must say
/// nothing. A client that closed its own socket, or whose link dropped, would
/// otherwise be told its session ended — and would park instead of recovering,
/// which is the mirror image of the bug this issue fixes.
///
/// Read from a WATCHER, because the client whose socket went away cannot report
/// what the daemon did or did not send it.
#[tokio::test]
async fn a_client_close_announces_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _shutdown) = start_daemon(dir.path()).await;

    let mut writer = page::console(
        port,
        json!({ "repo": "owner/workbench", "agent": "claude" }),
    )
    .await;
    writer.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut writer, "GOT:first", true).await;

    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = list.as_array().unwrap()[0]["id"].as_u64().unwrap();

    let mut watcher = page::console(port, json!({ "id": id, "watch": 1 })).await;
    read_until(&mut watcher, "GOT:first", false).await;

    // The WRITER goes away on its own: it closes its channel and its page
    // socket. The session lives on (the tmux model), so nobody was evicted and
    // there is nothing to announce.
    writer.close(None).await.unwrap();
    drop(writer);

    // Drain the watcher for a bounded window: it must see no `session-end`. The
    // stream must also stay OPEN — a watcher is not collateral of a detach.
    let quiet = tokio::time::timeout(Duration::from_millis(1500), async {
        while let Some(msg) = watcher.next().await {
            match msg {
                Ok(Message::Binary(bytes)) => {
                    if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                        if cmd.verb == "session-end" {
                            return Some(cmd);
                        }
                    }
                }
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        }
        None
    })
    .await;
    assert!(
        quiet.is_err(),
        "a client-initiated close must announce NOTHING and end no other stream, got {quiet:?}"
    );

    // Still live: the slot was released, so a plain reattach succeeds and drives.
    let mut again = page::console(port, json!({ "id": id })).await;
    again.send(terminal(b"still-here\r")).await.unwrap();
    read_until(&mut again, "GOT:still-here", false).await;

    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
}

/// The third reason on the wire: a daemon going down tells every attachment so,
/// and names itself rather than the child (`session_ws`'s `shutdown.changed()`
/// arm, which no other test drives).
#[tokio::test]
async fn daemon_shutdown_is_announced_as_such() {
    let dir = tempfile::tempdir().unwrap();
    let (port, shutdown) = start_daemon(dir.path()).await;

    let mut ws = page::console(
        port,
        json!({ "repo": "owner/workbench", "agent": "claude" }),
    )
    .await;
    ws.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut ws, "GOT:first", true).await;

    shutdown
        .send(true)
        .expect("the shutdown watch must accept a flip");

    let (ended, announced) = drain_capturing_announcement(&mut ws, 5).await;
    assert!(
        ended,
        "a shutting-down daemon must end the bridge, not hang it"
    );
    assert_eq!(
        announced.as_deref(),
        Some("daemon-shutdown"),
        "a daemon shutdown is its own reason — not the child exiting"
    );
}

/// A page sends its last keys and closes the console channel at once
/// (#834). The console reads the keys before it ends, and the daemon sends
/// no `channel.closed`: the page asked for the close.
#[tokio::test]
async fn keys_sent_just_before_a_channel_close_reach_the_console() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _shutdown) = start_daemon(dir.path()).await;

    let mut ws = page::console(
        port,
        json!({ "repo": "owner/workbench", "agent": "claude" }),
    )
    .await;
    ws.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut ws, "GOT:first", true).await;
    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = list.as_array().unwrap()[0]["id"].as_u64().unwrap();

    ws.send(terminal(b"last\r")).await.unwrap();
    ws.close_channel().await.unwrap();
    let closed = tokio::time::timeout(Duration::from_millis(1500), async {
        while let Some(message) = ws.next().await {
            if matches!(message, Ok(Message::Close(_))) {
                return;
            }
        }
    })
    .await;
    assert!(
        closed.is_err(),
        "no channel.closed: {:?}",
        ws.closed_reason()
    );

    let mut again = page::console(port, json!({ "id": id })).await;
    read_until(&mut again, "GOT:last", false).await;
    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
}
