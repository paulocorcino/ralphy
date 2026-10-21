use super::*;

/// A cursor-up redraw is right only at the width it was written at (#689).
/// Output written at 120 columns, then a resize to 60, then more output: the
/// replay, written into a new 60-column screen, gives the session's own
/// screen. The raw bytes would put the redraw in the wrong place.
#[test]
fn a_replay_rebuilds_the_screen_after_a_width_change() {
    let mut kept = KeptOutput::new(1, 24, 120);
    let mut wide = Vec::new();
    for i in 0..40 {
        let line = format!(
            "\x1b[3{}mline {i:03} {}\x1b[0m\r\n",
            i % 7 + 1,
            "x".repeat(100)
        );
        wide.extend_from_slice(line.as_bytes());
    }
    wide.extend_from_slice(b"\x1b[3A\x1b[100Cdone\r\n\r\n\r\n");
    kept.push(&wide);
    kept.resize(24, 60);
    kept.push(b"narrow after the resize\r\n");

    let mut fresh = Screen::new(24, 60, screen::SCROLLBACK_LINES);
    fresh.feed(&kept.replay(None).0);
    let session = kept.screen.as_ref().expect("no panic was caught").text();
    assert!(session.contains("done"), "the redraw is on the screen");
    assert_eq!(fresh.text(), session);
}

/// EXACTLY-ONCE REPLAY INVARIANT at the seam: a pump publishes numbered
/// lines while attaches happen on another thread. Every attach sees each line
/// exactly once, in its replay or on its live receiver.
#[test]
fn every_line_is_in_the_replay_or_on_the_live_receiver_once() {
    // A round where the scheduler ran every attach before or after the pump
    // proves nothing, so it is run again.
    let rounds = (0..5)
        .take_while(|_| attach_while_publishing() == 0)
        .count();
    assert!(
        rounds < 5,
        "no attach happened while the pump ran, in 5 rounds"
    );
}

/// One round of the test above. Returns how many attaches got a replay with
/// some, but not all, of the lines.
fn attach_while_publishing() -> usize {
    const LINES: usize = 800;
    let line = |i: usize| format!("L{i:04} {}", "-".repeat(70));
    let kept = Arc::new(Mutex::new(KeptOutput::new(1, 24, 80)));
    let (tx, _) = broadcast::channel(BROADCAST_CAP);
    let start = Arc::new(std::sync::Barrier::new(2));
    let pump = std::thread::spawn({
        let (kept, tx, start) = (kept.clone(), tx.clone(), start.clone());
        move || {
            start.wait();
            for i in 0..LINES {
                publish(&kept, &tx, format!("{}\r\n", line(i)).into_bytes());
            }
        }
    });
    start.wait();
    let mut attaches = Vec::new();
    // Each attach is checked below, and a check costs a screen; 100 attaches
    // keep the test short.
    while !pump.is_finished() && attaches.len() < 100 {
        attaches.push(replay_and_subscribe(&kept, &tx, None));
    }
    pump.join().expect("the pump thread does not panic");

    let expected: Vec<String> = (0..LINES).map(line).collect();
    let mut during = 0;
    for (n, Subscribed { replay, mut rx, .. }) in attaches.into_iter().enumerate() {
        let mut screen = Screen::new(24, 80, screen::SCROLLBACK_LINES);
        screen.feed(&replay);
        let mut seen: Vec<String> = screen
            .text()
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect();
        if (1..LINES).contains(&seen.len()) {
            during += 1;
        }
        while let Ok(chunk) = rx.try_recv() {
            let text = String::from_utf8(chunk).expect("the test writes ASCII");
            seen.extend(text.lines().map(str::to_owned));
        }
        assert_eq!(seen, expected, "attach {n}");
    }
    during
}

/// The writer and a watcher get the same replay: the screen snapshot.
#[tokio::test]
async fn a_watcher_gets_the_same_replay_as_the_writer() {
    let manager = Arc::new(SessionManager::new());
    let (id, writer) = manager
        .spawn_attached(
            "~".to_string(),
            "console".to_string(),
            "console".to_string(),
            None,
            None,
            None,
            console_spec(std::env::temp_dir(), 24, 80, None),
        )
        .expect("the platform shell must spawn");
    writer
        .write(b"echo ralphy\r")
        .expect("the shell takes input");
    drop(writer);
    // The shell may still be writing; two attaches are equal once it stops.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let writer = manager.attach(id, true).expect("a live session");
        let watcher = manager.watch(id, None).expect("a live session");
        if !writer.snapshot.is_empty() && writer.snapshot == watcher.snapshot {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the writer's and the watcher's replays never matched"
        );
        drop((writer, watcher));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    manager.close(id);
}

/// A panic in the emulator breaks the screen, and every later replay is the
/// raw ring (ADR-0079 D3). The ring keeps every byte, also the ones before
/// the panic.
#[test]
fn a_broken_screen_falls_back_to_the_raw_ring() {
    let mut kept = KeptOutput::new(1, 24, 80);
    kept.push(b"before\r\n");
    assert_ne!(
        kept.replay(None).0,
        b"before\r\n",
        "a working screen replays a snapshot"
    );
    assert!(kept.on_screen(|_| panic!("a test panic")).is_none());
    kept.push(b"after\r\n");
    assert!(kept.screen.is_none(), "the screen stays broken");
    assert_eq!(kept.replay(None).0, b"before\r\nafter\r\n");
}

/// The memory budget of ADR-0079 D4 is for screens that keep
/// `SCROLLBACK_LINES` lines: 30 sessions at 200 columns, each fed 512 KiB of
/// the harness's flood pattern, keep at most that many lines above their
/// visible rows, and a ring of at most 512 KiB.
#[test]
fn thirty_full_sessions_keep_only_the_scrollback_lines() {
    const ROWS: u16 = 50;
    let mut flood = Vec::new();
    let mut n = 0usize;
    while flood.len() < 512 * 1024 {
        n += 1;
        let line = |k: usize| {
            format!(
                "\x1b[3{}m{:<70}\x1b[0m\r\n",
                k % 7 + 1,
                format!("LINE {k} ")
            )
        };
        flood.extend_from_slice(line(n).as_bytes());
        if n.is_multiple_of(20) {
            flood.extend_from_slice(b"\x1b[3A");
            for again in n - 2..=n {
                flood.extend_from_slice(line(again).as_bytes());
            }
        }
    }
    for session in 0..30 {
        let mut kept = KeptOutput::new(session, ROWS, 200);
        for chunk in flood.chunks(8192) {
            kept.push(chunk);
        }
        let lines = kept
            .screen
            .as_ref()
            .expect("no panic")
            .text()
            .lines()
            .count();
        assert!(
            lines <= screen::SCROLLBACK_LINES + usize::from(ROWS),
            "session {session} keeps {lines} lines"
        );
        assert!(kept.ring.len() <= SCROLLBACK_CAP_BYTES);
    }
}

/// A resize reaches the PTY and the screen with the same clamped size
/// (ADR-0079 D3), so a replay is written at the session size.
#[tokio::test]
async fn a_resize_gives_the_screen_the_clamped_session_size() {
    let manager = Arc::new(SessionManager::new());
    let (id, writer) = manager
        .spawn_attached(
            "~".to_string(),
            "console".to_string(),
            "console".to_string(),
            None,
            None,
            None,
            console_spec(std::env::temp_dir(), 24, 80, None),
        )
        .expect("the platform shell must spawn");
    let sess = manager.sessions.lock().expect("sessions mutex")[&id].clone();
    let size = || {
        sess.kept
            .lock()
            .expect("kept output mutex")
            .screen
            .as_ref()
            .expect("no panic was caught")
            .size()
    };
    assert_eq!(size(), (24, 80), "the screen starts at the spawn size");
    writer.resize(30, 100).expect("the PTY takes a resize");
    assert_eq!(size(), (30, 100));
    writer
        .resize(1000, 1)
        .expect("the PTY takes a clamped resize");
    assert_eq!(size(), (300, 2));
    manager.close(id);
}

/// The session size goes to every attachment (ADR-0079 D6): a new one holds
/// the size now, a resize by the writer reaches a watcher, a resize to the
/// same size and a watcher's resize reach nobody, and the size outlives a
/// broken screen.
#[tokio::test]
async fn a_resize_by_the_writer_reaches_every_attachment() {
    let manager = Arc::new(SessionManager::new());
    let (id, writer) = manager
        .spawn_attached(
            "~".to_string(),
            "console".to_string(),
            "console".to_string(),
            None,
            None,
            None,
            console_spec(std::env::temp_dir(), 24, 80, None),
        )
        .expect("the platform shell must spawn");
    let mut watcher = manager
        .watch(id, None)
        .expect("a live session is watchable");
    assert_eq!(*watcher.size.borrow_and_update(), (24, 80));

    watcher
        .resize(40, 120)
        .expect("a watcher's resize is dropped");
    assert!(
        !watcher.size.has_changed().expect("the session is live"),
        "a watcher does not size the session"
    );

    writer.resize(30, 100).expect("the PTY takes a resize");
    assert!(watcher.size.has_changed().expect("the session is live"));
    assert_eq!(*watcher.size.borrow_and_update(), (30, 100));
    writer.resize(30, 100).expect("the PTY takes a resize");
    assert!(
        !watcher.size.has_changed().expect("the session is live"),
        "the same size is not sent again"
    );

    let sess = manager.sessions.lock().expect("sessions mutex")[&id].clone();
    let mut kept = sess.kept.lock().expect("kept output mutex");
    assert!(kept.on_screen(|_| panic!("a test panic")).is_none());
    drop(kept);
    writer
        .resize(1000, 1)
        .expect("the PTY takes a clamped resize");
    assert_eq!(*watcher.size.borrow_and_update(), (300, 2));
    let late = manager
        .watch(id, None)
        .expect("a live session is watchable");
    assert_eq!(*late.size.borrow(), (300, 2));
    manager.close(id);
}
