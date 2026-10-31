use super::*;

fn traffic(now: Instant) -> Traffic {
    let tab = Tab {
        holder: Some("tab-1".to_string()),
        ..Tab::default()
    };
    Traffic::local(1, tab, now)
}

#[test]
fn replay_and_live_bytes_are_kept_apart() {
    let mut t = traffic(Instant::now());
    t.replay(400, 1000);
    t.live(10);
    t.live(5);
    t.inbound(3);
    let s = t.summary();
    assert_eq!((s.replay_bytes, s.live_bytes, s.live_frames), (400, 15, 2));
    assert_eq!(s.replay_raw_bytes, 1000, "the replay before its gzip");
    assert_eq!(s.in_bytes, 3);
}

#[test]
fn the_keys_name_the_session_or_the_peer_and_the_tab() {
    let now = Instant::now();
    let device = DeviceId::mint();
    let tab = Tab {
        holder: Some("tab-a".to_string()),
        device: Some(device),
        ..Tab::default()
    };
    let local = Traffic::local(7, tab, now);
    assert_eq!(local.channel, Channel::Local);
    assert_eq!(local.keys.session, Some(7));
    assert_eq!(local.keys.holder.as_deref(), Some("tab-a"));
    assert_eq!(local.keys.device, Some(device));
    // The summary writes the ID alone: 32 hex digits, not the cookie
    // value with its version and HMAC.
    let written = device.to_string();
    assert_eq!(written.len(), 32);
    assert!(written.bytes().all(|b| b.is_ascii_hexdigit()), "{written}");
    let relayed = Traffic::peer(
        "01PEER".to_string(),
        "Ubuntu".to_string(),
        Tab::default(),
        now,
    );
    assert_eq!(relayed.channel, Channel::Peer);
    assert_eq!(relayed.keys.peer_id.as_deref(), Some("01PEER"));
    assert_eq!(relayed.keys.peer.as_deref(), Some("Ubuntu"));
    assert_eq!(relayed.keys.session, None);
}

#[test]
fn the_keys_carry_the_link_the_page_reported() {
    let link = Link {
        effective_type: Some(EffectiveType::G3),
        rtt_ms: Some(300),
        downlink_mbps: Some(1.5),
    };
    let tab = Tab {
        link,
        ..Tab::default()
    };
    assert_eq!(
        Traffic::local(7, tab.clone(), Instant::now()).keys.link,
        link
    );
    let relayed = Traffic::peer("01PEER".into(), "Mac".into(), tab, Instant::now());
    assert_eq!(relayed.keys.link, link);
    assert_eq!(traffic(Instant::now()).keys.link, Link::default(), "Safari");
}

fn command(json: &str) -> Command {
    match crate::protocol::decode(&[&[0x02], json.as_bytes()].concat()) {
        Ok(crate::protocol::Frame::Command(cmd)) => cmd,
        other => panic!("not a command frame: {other:?}"),
    }
}

#[test]
fn a_detach_command_names_the_leave() {
    // The bytes `encodeDetach` in wb-console.ts produces (its node test).
    let dormant = command(r#"{"id":0,"verb":"detach","payload":{"reason":"dormant"}}"#);
    assert_eq!(Leave::from_command(&dormant), Some(Leave::Dormant));
    let reconnect = command(r#"{"id":0,"verb":"detach","payload":{"reason":"reconnect"}}"#);
    assert_eq!(Leave::from_command(&reconnect), Some(Leave::Reconnect));
    let closed = command(r#"{"id":0,"verb":"detach","payload":{"reason":"window-closed"}}"#);
    assert_eq!(Leave::from_command(&closed), Some(Leave::WindowClosed));
    let hidden = command(r#"{"id":0,"verb":"detach","payload":{"reason":"hidden"}}"#);
    assert_eq!(Leave::from_command(&hidden), Some(Leave::Hidden));
    let unknown = command(r#"{"id":0,"verb":"detach","payload":{"reason":"bored"}}"#);
    assert_eq!(Leave::from_command(&unknown), None);
    let resize = command(r#"{"id":0,"verb":"resize","payload":{"reason":"dormant"}}"#);
    assert_eq!(
        Leave::from_command(&resize),
        None,
        "only `detach` names a leave"
    );
}

#[test]
fn the_end_is_the_most_specific_reason_known() {
    let mut t = traffic(Instant::now());
    assert_eq!(t.end_label(None), "dropped");
    t.client_command(&command(
        r#"{"id":0,"verb":"detach","payload":{"reason":"dormant"}}"#,
    ));
    assert_eq!(t.end_label(None), "dormant");
    assert_eq!(t.end_label(Some("taken-over")), "taken-over");
}

// The bytes `encodeDetach("hidden")` and `encodeShown` produce (their
// node test in wb-console-session.test.mjs).
const HIDDEN: &str = r#"{"id":0,"verb":"detach","payload":{"reason":"hidden"}}"#;
const SHOWN_AGAIN: &str = r#"{"id":0,"verb":"shown","payload":{}}"#;

#[test]
fn a_hidden_page_that_never_came_back_ends_hidden() {
    let mut t = traffic(Instant::now());
    t.client_command(&command(HIDDEN));
    assert_eq!(t.end_label(None), "hidden");
}

#[test]
fn a_page_shown_again_clears_hidden_but_not_dormant() {
    let mut t = traffic(Instant::now());
    t.client_command(&command(HIDDEN));
    t.client_command(&command(SHOWN_AGAIN));
    assert_eq!(t.end_label(None), "dropped", "a later failure is a drop");
    t.client_command(&command(
        r#"{"id":0,"verb":"detach","payload":{"reason":"dormant"}}"#,
    ));
    t.client_command(&command(SHOWN_AGAIN));
    assert_eq!(t.end_label(None), "dormant");
}

fn peer() -> Traffic {
    Traffic::peer(
        "01PEER".to_string(),
        "Mac".to_string(),
        Tab::default(),
        Instant::now(),
    )
}

fn open_frame(payload: &str) -> Vec<u8> {
    [
        &[protocol::TAG_COMMAND][..],
        format!(r#"{{"id":4,"verb":"session-open","payload":{payload}}}"#).as_bytes(),
    ]
    .concat()
}

fn terminal_frame(len: usize) -> Vec<u8> {
    protocol::encode(&Frame::Terminal {
        session: 4,
        data: vec![b'x'; len],
    })
}

#[test]
fn a_relayed_replay_is_counted_as_replay_and_names_the_session() {
    let mut t = peer();
    t.peer_frame(&open_frame(r#"{"session":4,"replay":true}"#));
    let replay = terminal_frame(1000);
    t.peer_frame(&replay);
    t.peer_frame(&terminal_frame(10));
    let s = t.summary();
    assert_eq!(t.keys.session, Some(4));
    assert_eq!(s.replay_bytes, replay.len() as u64);
    assert_eq!(s.replay_raw_bytes, replay.len() as u64, "a raw replay");
    assert_eq!(s.live_frames, 1, "the open frame is not counted");
    assert_eq!(s.live_bytes, terminal_frame(10).len() as u64);
}

/// A peer that gzips its replay sends a `session-size` frame between the
/// announcement and the replay; the replay is still found, and its raw
/// size is the size of the frame decoded.
#[test]
fn a_relayed_gzip_replay_is_counted_on_the_wire_and_raw() {
    use std::io::Write;
    let mut t = peer();
    t.peer_frame(&open_frame(
        r#"{"session":4,"replay":true,"replay_encoding":"gzip"}"#,
    ));
    let size = [
        &[protocol::TAG_COMMAND][..],
        br#"{"id":4,"verb":"session-size","payload":{"rows":24,"cols":80}}"#,
    ]
    .concat();
    t.peer_frame(&size);
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&[b'x'; 5000]).expect("an in-memory gzip");
    let replay = protocol::encode(&Frame::Terminal {
        session: 4,
        data: enc.finish().expect("an in-memory gzip"),
    });
    t.peer_frame(&replay);
    t.peer_frame(&terminal_frame(10));
    let s = t.summary();
    assert_eq!(s.replay_bytes, replay.len() as u64);
    assert_eq!(s.replay_raw_bytes, terminal_frame(5000).len() as u64);
    assert_eq!(s.live_frames, 2, "the size frame and the live frame");
}

/// A peer that lagged sends a second `session-open` and a new snapshot:
/// that snapshot is a replay too, not live output.
#[test]
fn a_relayed_resync_counts_its_snapshot_as_replay() {
    let mut t = peer();
    t.peer_frame(&open_frame(r#"{"session":4,"replay":true}"#));
    t.peer_frame(&terminal_frame(1000));
    t.peer_frame(&terminal_frame(10));
    t.peer_frame(&open_frame(r#"{"session":4,"replay":true}"#));
    t.peer_frame(&terminal_frame(2000));
    let s = t.summary();
    assert_eq!(
        s.replay_bytes,
        (terminal_frame(1000).len() + terminal_frame(2000).len()) as u64
    );
    assert_eq!(
        (s.live_frames, s.live_bytes),
        (1, terminal_frame(10).len() as u64)
    );
}

#[test]
fn an_older_peer_without_the_replay_field_still_names_the_session() {
    let mut t = peer();
    t.peer_frame(&open_frame(r#"{"session":4}"#));
    t.peer_frame(&terminal_frame(1000));
    let s = t.summary();
    assert_eq!(t.keys.session, Some(4));
    assert_eq!((s.replay_bytes, s.live_frames), (0, 1));
}

#[test]
fn a_burst_that_outran_the_socket_is_counted() {
    let mut t = traffic(Instant::now());
    t.lagged(7);
    t.lagged(3);
    let s = t.summary();
    assert_eq!((s.lagged_events, s.lagged_skipped), (2, 10));
}

/// A writer the tests read back: what one `Traffic::log` call wrote.
#[derive(Clone, Default)]
struct Written(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Written {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("a test lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Written {
    type Writer = Written;
    fn make_writer(&'a self) -> Written {
        self.clone()
    }
}

fn logged(t: &Traffic) -> String {
    let written = Written::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(written.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || t.log(None, Instant::now()));
    let bytes = written.0.lock().expect("a test lock").clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn the_traffic_line_names_the_link_and_safari_has_none() {
    let tab = Tab {
        link: Link {
            effective_type: Some(EffectiveType::G4),
            rtt_ms: Some(100),
            downlink_mbps: Some(1.75),
        },
        ..Tab::default()
    };
    let line = logged(&Traffic::local(7, tab, Instant::now()));
    assert!(line.contains("console socket traffic"), "{line}");
    for field in [
        r#"net_type="4g""#,
        "net_rtt_ms=100",
        "net_downlink_mbps=1.75",
    ] {
        assert!(line.contains(field), "{field} not in {line}");
    }
    let safari = logged(&traffic(Instant::now()));
    assert!(safari.contains("console socket traffic"), "{safari}");
    assert!(!safari.contains("net_"), "{safari}");
}
