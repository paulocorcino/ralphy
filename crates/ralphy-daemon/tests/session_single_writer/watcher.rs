use super::*;

/// `id: N, watch: 1` (issue #334): a second client reaches a BUSY session as a
/// reader — no refusal, no eviction — sees the same replay and the same live
/// stream, and its keystrokes and resizes never reach the child. The read half
/// is what makes "both contexts see the session's output" true; the write half
/// is what makes a watcher a watcher rather than a second writer.
#[tokio::test]
async fn a_watcher_reads_but_never_writes() {
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

    // An unknown id is refused even as a watcher — `watch` maps only `Unknown`.
    refused(
        port,
        json!({ "id": id + 9999, "watch": 1 }),
        "a watch on an unknown id is refused",
    )
    .await;

    // The writer still holds the slot, so a plain `id` here would be refused.
    let mut watcher = page::console(port, json!({ "id": id, "watch": 1 })).await;
    let replay = read_until(&mut watcher, "GOT:first", false).await;
    assert!(
        replay.contains("GOT:first"),
        "a watcher gets the same scrollback replay as a writer, got {replay:?}"
    );

    // The watcher types and resizes. Neither may reach the child.
    watcher.send(terminal(b"watcher-typing\r")).await.unwrap();
    watcher.send(resize(99, 200)).await.unwrap();

    // Settle BEFORE the writer speaks, or the oracle is vacuous by race: the
    // reader below stops at the writer's own line, so a watcher echo that merely
    // arrived LATER would never be looked at. Measured: without this wait the
    // assertions still passed with the watcher gate deleted. 600ms is ~12 polls
    // of the child's 50ms size ticker and orders of magnitude over a loopback
    // PTY round-trip.
    tokio::time::sleep(Duration::from_millis(600)).await;

    // The oracle is read on the WRITER's stream, and it is live: the writer's own
    // keystroke round-trips right after, so "the watcher's line is absent" cannot
    // pass merely because nothing was flowing.
    writer.send(terminal(b"after-watch\r")).await.unwrap();
    let seen = read_until(&mut writer, "GOT:after-watch", false).await;
    assert!(
        !seen.contains("GOT:watcher-typing"),
        "a watcher's keystroke must never reach the child, got {seen:?}"
    );
    assert!(
        !seen.contains("SIZE 200x99"),
        "a watcher's resize must never reach the PTY, got {seen:?}"
    );
    // …and the resize oracle itself is live: the WRITER's resize does land.
    writer.send(resize(40, 100)).await.unwrap();
    read_until(&mut writer, "SIZE 100x40", false).await;
    // The PTY gets the size clamped to 300 rows and 500 columns, as the
    // daemon's screen does; then the test goes back to 40x100.
    writer.send(resize(u16::MAX, u16::MAX)).await.unwrap();
    read_until(&mut writer, "SIZE 500x300", false).await;
    writer.send(resize(40, 100)).await.unwrap();
    read_until(&mut writer, "SIZE 100x40", false).await;

    // `watch: 1` is dispatched BEFORE the attach branch, so a stray `takeover: 1`
    // riding along cannot evict the writer. Swapping that order reds this.
    let mut sneak = page::console(port, json!({ "id": id, "watch": 1, "takeover": 1 })).await;
    writer.send(terminal(b"still-mine\r")).await.unwrap();
    read_until(&mut writer, "GOT:still-mine", false).await;
    // The channel was accepted as a watch: it shows the same output.
    read_until(&mut sneak, "GOT:still-mine", false).await;
    drop(sneak.close(None).await);

    // A takeover evicts the WRITER and leaves the watcher untouched: exactly one
    // writer, and watching is not a claim on the slot.
    let mut taker = page::console(port, json!({ "id": id, "takeover": 1 })).await;
    let (ended, announced) = drain_capturing_announcement(&mut writer, 5).await;
    assert!(ended, "the evicted writer's stream must end");
    assert_eq!(announced.as_deref(), Some("taken-over"));
    assert_eq!(
        writer.closed_reason(),
        Some("done"),
        "the daemon closed the writer"
    );

    // The evicted incumbent's guard-drop must not clear the TAKER's slot: asked
    // again AFTER the eviction has settled, a plain attach is still refused.
    // (Before the takeover the same probe passes even with the `ptr_eq` guard
    // removed, which is why it is asked here.)
    refused(
        port,
        json!({ "id": id }),
        "the taker must still hold the slot after the incumbent's guard dropped",
    )
    .await;

    taker.send(terminal(b"after-takeover\r")).await.unwrap();
    read_until(&mut taker, "GOT:after-takeover", false).await;
    let watched = read_until(&mut watcher, "GOT:after-takeover", false).await;
    assert!(
        watched.contains("GOT:after-takeover"),
        "the watcher's stream survives a takeover and keeps showing the session, got {watched:?}"
    );

    // A genuine end reaches the watcher too, named as such.
    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
    let (watcher_ended, watcher_told) = drain_capturing_announcement(&mut watcher, 5).await;
    assert!(watcher_ended, "the watcher's stream must end on a close");
    assert_eq!(
        watcher_told.as_deref(),
        Some("child-exited"),
        "a watcher is told the session ended, not left on a dead socket"
    );
    assert_eq!(
        watcher.closed_reason(),
        Some("done"),
        "the daemon closed the watcher"
    );
}
