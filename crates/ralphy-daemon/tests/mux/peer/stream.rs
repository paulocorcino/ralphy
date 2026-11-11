//! A peer console on a channel: it streams and resizes, its replay is cut into pieces, a
//! ping is answered locally, and two sessions with one id are two channels.

use super::*;

#[tokio::test]
async fn a_peer_console_on_a_channel_streams_and_resizes() {
    let fleet = fleet().await;
    let mut page = Page::connect(&fleet).await;
    let opened = page.launch(1, &peer_repo()).await;
    assert_eq!(opened.payload["daemon_id"], PEER_ID, "{opened:?}");
    assert!(
        names_cwd(&page.shown(1), fleet.peer_repo.path()),
        "the console runs in the peer's checkout: {}",
        page.shown(1)
    );
    let id = session_of(&opened);

    page.type_line(1, id, "peer-input").await;
    page.until(1, "GOT:peer-input").await;

    let resize = Frame::Command(Command {
        id: 0,
        verb: "resize".to_owned(),
        payload: serde_json::json!({ "rows": 40, "cols": 100 }),
    });
    page.send(1, &resize).await;
    page.until(1, "SIZE 100x40").await;
}

#[tokio::test]
async fn a_peer_replay_is_cut_into_pieces() {
    let fleet = fleet().await;
    let mut page = Page::connect(&fleet).await;
    let opened = page.launch(1, &peer_repo()).await;
    let id = session_of(&opened);
    let since = opened.payload["offset"].as_u64().expect("a byte count");
    // A terminal that draws by frames (ConPTY) writes only part of a fast
    // flood, so the child floods until the session wrote 100 KiB.
    let mut floods = 0;
    while page.text.get(&1).map_or(0, Vec::len) < 100 * 1024 {
        floods += 1;
        page.type_line(1, id, "flood 100").await;
        page.until_count(1, "FLOOD-DONE", floods).await;
    }
    page.type_line(1, id, "after-flood").await;
    page.until(1, "GOT:after-flood").await;
    let live = page.text.remove(&1).unwrap_or_default();
    close(&mut page.ws, 1).await;

    let fields = serde_json::json!({ "id": id, "repo": peer_repo(), "watch": 1, "since": since });
    open(&mut page.ws, 2, "console", fields).await;
    page.command(2, "session-open").await;
    page.command(2, "session-size").await;
    let replay = loop {
        let inner = page.next().await;
        if inner.channel == 2 {
            break inner;
        }
    };
    let Frame::Terminal { data, .. } = replay.frame else {
        panic!("expected the replay on channel 2, got {:?}", replay.frame);
    };
    assert!(
        replay.pieces.len() > 1,
        "a replay of 100 KiB comes in pieces: {:?}",
        replay.pieces
    );
    assert!(
        replay.pieces.iter().all(|len| *len <= PIECE_CAP),
        "every piece is at most {PIECE_CAP} bytes: {:?}",
        replay.pieces
    );
    assert!(
        data.starts_with(&live),
        "the pieces join to the bytes the session wrote: {} joined, {} live",
        data.len(),
        live.len()
    );
}

#[tokio::test]
async fn a_peer_ping_is_answered_by_the_local_daemon() {
    fast_pings();
    let fleet = fleet().await;
    let mut page = Page::connect(&fleet).await;
    let opened = page.launch(1, &peer_repo()).await;
    let id = session_of(&opened);

    // The page reads nothing for many of the peer's silent windows, so it
    // answers no ping. It sends pings of its own, which its daemon answers,
    // so its own socket stays heard.
    let quiet_until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < quiet_until {
        page.ws
            .send(Message::Ping(Vec::new().into()))
            .await
            .expect("send a ping on the page socket");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    page.type_line(1, id, "still-here").await;
    page.until(1, "GOT:still-here").await;
    assert_eq!(page.peer_pings, 0, "no peer ping reaches the page");
}

#[tokio::test]
async fn two_daemons_with_the_same_session_id_are_two_channels() {
    let fleet = fleet().await;
    let mut page = Page::connect(&fleet).await;
    let local = page.launch(1, SLUG).await;
    let remote = page.launch(2, &peer_repo()).await;
    assert_eq!(local.payload["daemon_id"], LOCAL_ID, "{local:?}");
    assert_eq!(remote.payload["daemon_id"], PEER_ID, "{remote:?}");
    let id = session_of(&local);
    assert_eq!(session_of(&remote), id, "each daemon gave its first id");
    assert!(names_cwd(&page.shown(1), fleet.local_repo.path()));
    assert!(names_cwd(&page.shown(2), fleet.peer_repo.path()));

    page.type_line(1, id, "on-local").await;
    page.type_line(2, id, "on-peer").await;
    page.until(1, "GOT:on-local").await;
    page.until(2, "GOT:on-peer").await;
    assert!(!page.shown(1).contains("GOT:on-peer"), "{}", page.shown(1));
    assert!(!page.shown(2).contains("GOT:on-local"), "{}", page.shown(2));
}
