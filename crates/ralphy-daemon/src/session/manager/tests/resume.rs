use super::*;

/// A kept output that wrote `a`, then `b`: the byte count after `a` is
/// `a.len()`.
fn kept_after(a: &[u8], b: &[u8]) -> KeptOutput {
    let mut kept = KeptOutput::new(1, 24, 80);
    kept.push(a);
    kept.push(b);
    kept
}

/// ADR-0079 D8: a `since` inside the ring resumes with exactly the bytes
/// after it; a `since` at the count resumes with nothing.
#[test]
fn a_since_inside_the_ring_resumes_with_the_bytes_after_it() {
    let mut kept = kept_after(b"first line\r\n", b"second\r\n");
    assert_eq!(kept.replay(Some(12)), (b"second\r\n".to_vec(), true));
    assert_eq!(kept.replay(Some(13)), (b"econd\r\n".to_vec(), true));
    assert_eq!(kept.replay(Some(20)), (Vec::new(), true), "at the count");
    assert_eq!(
        kept.replay(Some(0)),
        (b"first line\r\nsecond\r\n".to_vec(), true),
        "the ring still has every byte"
    );
}

/// No `since`, a `since` after the count, and a `since` before the last
/// resize all get the screen snapshot, not a resume.
#[test]
fn a_since_that_cannot_resume_gets_the_snapshot() {
    let mut kept = kept_after(b"first line\r\n", b"second\r\n");
    let (snapshot, resume) = kept.replay(None);
    assert!(!resume);
    assert_ne!(
        snapshot, b"first line\r\nsecond\r\n",
        "a snapshot, not the ring"
    );
    assert_eq!(
        kept.replay(Some(21)),
        (snapshot.clone(), false),
        "after the count"
    );

    kept.resize(24, 60);
    kept.push(b"narrow\r\n");
    let (narrow, resume) = kept.replay(Some(12));
    assert!(
        !resume,
        "bytes before the resize were written at another size"
    );
    let mut fresh = Screen::new(24, 60, screen::SCROLLBACK_LINES);
    fresh.feed(&narrow);
    assert!(
        fresh.text().contains("narrow"),
        "the snapshot of the screen now"
    );
    assert_eq!(
        kept.replay(Some(20)),
        (b"narrow\r\n".to_vec(), true),
        "at the resize, every byte after it is at the new size"
    );
}

/// A resize to the size the session has changes nothing: the snapshot still
/// writes the scroll region, and a `since` from before it still resumes. The
/// page sends its size on every socket open, and on Linux and macOS the child
/// gets no signal for the same size, so it never writes the region again.
#[test]
fn a_resize_to_the_same_size_keeps_the_region_and_the_resume() {
    let mut kept = kept_after(b"\x1b[2;20r", b"body\r\n");
    kept.resize(24, 80);
    let (snapshot, resume) = kept.replay(None);
    assert!(!resume);
    assert!(
        snapshot.windows(7).any(|w| w == b"\x1b[2;20r"),
        "the snapshot keeps the scroll region"
    );
    assert_eq!(
        kept.replay(Some(7)),
        (b"body\r\n".to_vec(), true),
        "a resume from before the same-size resize"
    );
}

/// A `since` before the first byte the ring still has gets the snapshot: the
/// bytes between are gone.
#[test]
fn a_since_before_the_ring_start_gets_the_snapshot() {
    let line = |i: usize| format!("{i:07} {}\r\n", "x".repeat(70));
    let mut kept = KeptOutput::new(1, 24, 80);
    let mut written = 0usize;
    let mut i = 0;
    while written <= SCROLLBACK_CAP_BYTES + 1000 {
        let bytes = line(i).into_bytes();
        written += bytes.len();
        kept.push(&bytes);
        i += 1;
    }
    let start = (written - SCROLLBACK_CAP_BYTES) as u64;
    assert!(!kept.replay(Some(start - 1)).1, "one byte before the ring");
    let (tail, resume) = kept.replay(Some(start));
    assert!(resume, "the first byte the ring has");
    assert_eq!(tail.len(), SCROLLBACK_CAP_BYTES);
}

/// After a panic of the emulator the replay of a plain attach is the raw
/// ring, and a resume is still the bytes after `since`.
#[test]
fn a_resume_still_works_after_the_screen_broke() {
    let mut kept = kept_after(b"before\r\n", b"after\r\n");
    assert!(kept.on_screen(|_| panic!("a test panic")).is_none());
    assert_eq!(kept.replay(Some(8)), (b"after\r\n".to_vec(), true));
}

/// EXACTLY-ONCE REPLAY INVARIANT with `since`: attaches that resume at one
/// byte count while a pump publishes get, in their tail and then on their
/// live receiver, exactly the bytes after that count, and an `offset` at the
/// end of the tail. The pump waits for one attach in the middle, so the test
/// does not depend on the scheduler.
#[test]
fn a_resume_while_publishing_gets_every_byte_after_since_once() {
    const LINES: usize = 800;
    let line = |i: usize| format!("L{i:04} {}\r\n", "-".repeat(70)).into_bytes();
    let kept = Arc::new(Mutex::new(KeptOutput::new(1, 24, 80)));
    let (tx, _) = broadcast::channel(BROADCAST_CAP);
    for i in 0..10 {
        publish(&kept, &tx, line(i));
    }
    let since = replay_and_subscribe(&kept, &tx, None).offset;
    let chunks: Vec<Vec<u8>> = (10..LINES).map(line).collect();
    let half: Vec<u8> = chunks[..chunks.len() / 2].concat();
    let (middle, later) = attach_during_a_pump(&kept, &tx, chunks, 200, || {
        replay_and_subscribe(&kept, &tx, Some(since))
    });

    assert_eq!(
        middle.replay, half,
        "the middle attach resumes with the first half"
    );
    let all: Vec<u8> = (0..LINES).flat_map(line).collect();
    let expected = &all[usize::try_from(since).expect("a small count")..];
    for (n, mut sub) in std::iter::once(middle).chain(later).enumerate() {
        assert!(sub.resume, "attach {n} resumes");
        assert_eq!(sub.offset, since + sub.replay.len() as u64, "attach {n}");
        let mut seen = sub.replay;
        while let Ok(chunk) = sub.rx.try_recv() {
            seen.extend(chunk);
        }
        assert_eq!(seen, expected, "attach {n}");
    }
}

/// An attach names the byte count and whether it resumed: a reattach at the
/// count of the last one resumes; one with no `since` does not.
#[tokio::test]
async fn an_attachment_says_its_offset_and_whether_it_resumed() {
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
    assert!(!writer.resume, "a launch starts a new terminal");
    let offset = writer.offset;
    drop(writer);
    let resumed = manager
        .attach_as(id, false, None, None, Some(offset))
        .expect("a free slot");
    assert!(resumed.resume);
    assert_eq!(resumed.offset, offset + resumed.snapshot.len() as u64);
    let watcher = manager.watch(id, None).expect("a live session");
    assert!(!watcher.resume);
    manager.close(id);
}

/// A watcher whose receiver lagged starts again from the screen: after
/// `resync`, its new replay plus its new receiver show what the session
/// shows, and the lost bytes are not missed.
#[tokio::test]
async fn a_resync_after_a_lag_shows_the_session_screen() {
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
    drop(writer);
    let mut watcher = manager.watch(id, None).expect("a live session");
    let line = |i: usize| format!("R{i:04} {}\r\n", "-".repeat(60)).into_bytes();
    for i in 0..BROADCAST_CAP + 10 {
        publish(&watcher.sess.kept, &watcher.sess.tx, line(i));
    }
    assert!(matches!(
        watcher.rx.try_recv(),
        Err(broadcast::error::TryRecvError::Lagged(_))
    ));

    watcher.resync();
    assert!(!watcher.resume, "a resync is never a resume");
    for i in 0..5 {
        publish(
            &watcher.sess.kept,
            &watcher.sess.tx,
            format!("after {i}\r\n").into_bytes(),
        );
    }
    // Under the lock no chunk is on its way, so the receiver holds every
    // byte up to the count.
    let kept = watcher.sess.kept.lock().expect("kept output mutex");
    let (rows, cols) = *watcher.size.borrow();
    let mut page = Screen::new(rows, cols, screen::SCROLLBACK_LINES);
    page.feed(&watcher.snapshot);
    let mut live = 0u64;
    while let Ok(chunk) = watcher.rx.try_recv() {
        live += chunk.len() as u64;
        page.feed(&chunk);
    }
    assert_eq!(
        watcher.offset + live,
        kept.written,
        "no byte is lost or repeated"
    );
    let session = kept.screen.as_ref().expect("no panic was caught").text();
    assert!(session.contains("after 4"));
    assert_eq!(page.text(), session);
    drop(kept);
    manager.close(id);
}
