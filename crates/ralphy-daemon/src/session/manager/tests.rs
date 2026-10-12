use super::kept::{push_capped, SCROLLBACK_CAP_BYTES};
use super::*;
use crate::session::console_spec;
use crate::session::screen::Screen;

#[test]
fn scrollback_ring_is_bounded() {
    let mut ring = std::collections::VecDeque::new();
    // Fed in ≥2 chunks: proves the cap holds across successive pushes, not
    // only on a single oversized write.
    push_capped(&mut ring, b"012345", 8);
    push_capped(&mut ring, b"6789AB", 8);
    assert_eq!(ring.len(), 8, "ring must not exceed the cap");
    assert_eq!(
        ring.iter().copied().collect::<Vec<u8>>(),
        b"456789AB".to_vec(),
        "the FRONT is dropped and the tail retained"
    );
}

/// The replay of a reattach is the ring; a cap of 256 KiB gave 407 of 600
/// measured reattaches a full ring (#689).
#[test]
fn scrollback_cap_is_512_kib() {
    assert_eq!(SCROLLBACK_CAP_BYTES, 512 * 1024);
}

/// The writer slot and the watcher list are separate registers: a watcher may
/// never make the slot LOOK occupied (which would refuse an honest attach with
/// `409`), and it must be reachable while the slot IS held (which is the whole
/// point — a second workbench sees the session instead of stealing it).
///
/// Spawns the platform shell rather than the helper child bin: `CARGO_BIN_EXE_*`
/// is visible only to integration tests (docs/TESTING.md → What may be faked), and
/// nothing here talks to the child — the session only has to be LIVE.
#[tokio::test]
async fn a_watcher_does_not_occupy_the_writer_slot() {
    let manager = Arc::new(SessionManager::new());
    let spec = console_spec(std::env::temp_dir(), 24, 80, None);
    let (id, writer) = manager
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
    drop(writer); // the slot is free; only the watcher list is about to fill

    let watcher = manager
        .watch(id, None)
        .expect("a live session is always watchable");
    assert!(!watcher.writer, "watch() yields a reader, never the baton");
    let taker = manager
        .attach(id, false)
        .expect("a registered watcher must not make the writer slot look busy");
    assert!(taker.writer, "a plain attach takes the baton");
    let second = manager
        .watch(id, None)
        .expect("a BUSY session is still watchable — watch never refuses with Busy");
    assert!(!second.writer);

    // A departing watcher deregisters EXACTLY its own token: drop the first
    // and the second must still be told the session ended. Inverting the
    // guard's `ptr_eq` (keep the departing one, drop everyone else) makes
    // this reason `None`.
    let survivor = second.evict.clone();
    drop(watcher);
    manager.close(id);
    assert_eq!(
        survivor.reason(),
        Some(EndReason::ChildExited),
        "a watcher that outlives another must still receive the end signal"
    );
    assert_eq!(
        taker.evict.reason(),
        Some(EndReason::ChildExited),
        "…and so must the writer"
    );
}

/// ADR-0059 §6 through the listing, not the pure `render`: a `working`
/// observed longer than `STALE_AFTER` ago lists as `unknown`, a fresh one
/// as `working`, a `waiting` never ages — and `list` reads the observation
/// the pump stored, so replacing the render with a bare clone would red.
/// A status-less console (a shell) lists no state at all.
#[tokio::test]
async fn list_renders_the_agent_state_with_the_staleness_rule() {
    use crate::agent_state::{Observed, STALE_AFTER};
    let manager = Arc::new(SessionManager::new());
    let (id, _att) = manager
        .spawn_attached(
            "owner/r".to_string(),
            "claude".to_string(),
            "agent".to_string(),
            None,
            None,
            None,
            console_spec(std::env::temp_dir(), 24, 80, None),
        )
        .expect("the platform shell must spawn");
    let now = SystemTime::now();
    assert_eq!(
        manager.list_at(now)[0].agent_state,
        None,
        "no hook fired, no state"
    );
    let observe = |state: &'static str, seen: SystemTime| {
        let sess = manager.sessions.lock().expect("sessions mutex")[&id].clone();
        *sess.agent_state.lock().expect("agent_state mutex") = Some(Observed {
            state,
            detail: None,
            since: "t".into(),
            seen,
        });
    };
    observe(
        "working",
        now - STALE_AFTER + std::time::Duration::from_secs(60),
    );
    assert_eq!(
        manager.list_at(now)[0]
            .agent_state
            .as_ref()
            .map(|a| a.state.as_str()),
        Some("working")
    );
    observe(
        "working",
        now - STALE_AFTER - std::time::Duration::from_secs(60),
    );
    assert_eq!(
        manager.list_at(now)[0]
            .agent_state
            .as_ref()
            .map(|a| a.state.as_str()),
        Some("unknown")
    );
    assert_eq!(
        manager.get(id).and_then(|i| i.agent_state).map(|a| a.state),
        Some("unknown".to_string()),
        "`get` renders too"
    );
    observe(
        "waiting",
        now - STALE_AFTER - std::time::Duration::from_secs(60),
    );
    assert_eq!(
        manager.list_at(now)[0]
            .agent_state
            .as_ref()
            .map(|a| a.state.as_str()),
        Some("waiting"),
        "a question never goes stale"
    );
    manager.close(id);
}

/// `console_in` is the (repo, checkout) pair, exactly: another repo's
/// console in a same-named worktree, or this repo's console in the primary
/// (`checkout: None`), never gates a remove. Spawns the platform shell for
/// the same reason as the watcher test above.
#[tokio::test]
async fn console_in_matches_only_the_repo_and_checkout_pair() {
    let manager = Arc::new(SessionManager::new());
    let spawn = |repo: &str, checkout: Option<&str>| {
        manager
            .spawn_attached(
                repo.to_string(),
                "claude".to_string(),
                "agent".to_string(),
                None,
                checkout.map(str::to_string),
                None,
                console_spec(std::env::temp_dir(), 24, 80, None),
            )
            .expect("the platform shell must spawn")
    };
    assert!(!manager.console_in("owner/r", "wt-a"), "empty table");
    let (in_wt, _w1) = spawn("owner/r", Some("wt-a"));
    let (_primary, _w2) = spawn("owner/r", None);
    let (_other, _w3) = spawn("owner/other", Some("wt-a"));
    assert!(manager.console_in("owner/r", "wt-a"));
    assert!(
        !manager.console_in("owner/r", "wt-b"),
        "a different worktree"
    );
    assert!(
        !manager.console_in("owner/x", "wt-a"),
        "a same-named worktree of another repo"
    );
    manager.close(in_wt);
    assert!(
        !manager.console_in("owner/r", "wt-a"),
        "closed: the primary's console (checkout None) is not a match"
    );
    for id in manager.list().iter().map(|s| s.id) {
        manager.close(id);
    }
}

/// A restarted daemon must not re-issue an id the last one used: a browser
/// still holding `?id=N` would otherwise reattach to whichever NEW session drew
/// N. A manager over the same record continues past it; a first boot starts at 1.
#[test]
fn a_manager_over_the_same_record_continues_past_the_last_id() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let file = dir.path().join("daemon-session-id");

    let first = SessionManager::continuing(file.clone());
    assert_eq!(first.reserve_id(), 1, "a first boot starts at 1");
    assert_eq!(first.reserve_id(), 2);
    drop(first);

    let restarted = SessionManager::continuing(file.clone());
    assert_eq!(
        restarted.reserve_id(),
        3,
        "a restart continues past the record"
    );
    assert_eq!(
        std::fs::read_to_string(&file).expect("the record").trim(),
        "3"
    );
}

/// A corrupt record restarts the sequence rather than refusing sessions.
#[test]
fn a_corrupt_record_restarts_at_one() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let file = dir.path().join("daemon-session-id");
    std::fs::write(&file, "not a number").expect("seeding the record");
    assert_eq!(SessionManager::continuing(file).reserve_id(), 1);
}

/// The pump's tick reports a change in what the list shows, so a `working`
/// that ages into `unknown` with no new hook line is a change too: it pushes
/// `sessions.dirty`, and a tab does not keep showing a hung agent as working.
#[tokio::test]
async fn a_working_state_that_ages_out_counts_as_a_change() {
    use crate::agent_state::{Observed, StatusFiles, STALE_AFTER};
    let manager = Arc::new(SessionManager::new());
    let dir = std::env::temp_dir().join(format!("ralphy-age-out-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut spec = console_spec(std::env::temp_dir(), 24, 80, None);
    spec.status = Some(StatusFiles::for_session(&dir, 1));
    let (id, _att) = manager
        .spawn_attached(
            "owner/r".to_string(),
            "claude".to_string(),
            "agent".to_string(),
            None,
            None,
            None,
            spec,
        )
        .expect("the platform shell must spawn");
    let sess = manager.sessions.lock().expect("sessions mutex")[&id].clone();
    let now = SystemTime::now();
    *sess.agent_state.lock().expect("agent_state mutex") = Some(Observed {
        state: "working",
        detail: None,
        since: "t".into(),
        seen: now - STALE_AFTER + std::time::Duration::from_secs(1),
    });
    assert!(sess.poll_status(now), "the first state shown is a change");
    assert!(!sess.poll_status(now), "the same state again is not");
    let later = now + std::time::Duration::from_secs(2);
    assert!(
        sess.poll_status(later),
        "working aged into unknown with no new line is a change"
    );
    assert!(!sess.poll_status(later), "unknown again is not");
    manager.close(id);
    std::fs::remove_dir_all(&dir).ok();
}

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

/// A live console session whose writer slot `holder` claimed with `attempt`.
fn held_session(
    holder: &str,
    attempt: Option<u64>,
) -> (Arc<SessionManager>, SessionId, Attachment) {
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
    writer.hold_as(holder, attempt);
    (manager, id, writer)
}

/// ADR-0079 D7: the claims of one holder are ordered by their attempt. A
/// lower one is a late socket the tab already replaced: it is refused and
/// evicts nobody, also with `takeover`. An equal or higher one reclaims the
/// slot, the half-open case of ADR-0051 §9.
#[tokio::test]
async fn a_lower_attempt_of_the_same_holder_is_superseded_and_evicts_nobody() {
    let (manager, id, writer) = held_session("tab-A", Some(5));
    for takeover in [false, true] {
        assert!(
            matches!(
                manager.attach_as(id, takeover, Some("tab-A"), Some(4), None),
                Err(AttachError::Superseded)
            ),
            "attempt 4 after 5 is a late socket (takeover {takeover})"
        );
    }
    assert_eq!(writer.evict.reason(), None, "the writer is not evicted");

    let newer = manager
        .attach_as(id, false, Some("tab-A"), Some(6), None)
        .expect("a higher attempt of the same holder reclaims the slot");
    assert_eq!(writer.evict.reason(), Some(EndReason::TakenOver));
    let equal = manager
        .attach_as(id, false, Some("tab-A"), Some(6), None)
        .expect("an equal attempt reclaims the slot too");
    assert_eq!(newer.evict.reason(), Some(EndReason::TakenOver));
    assert!(
        matches!(
            manager.attach_as(id, false, Some("tab-A"), Some(5), None),
            Err(AttachError::Superseded)
        ),
        "the reclaim keeps the attempt it claimed with"
    );
    assert_eq!(equal.evict.reason(), None);
    manager.close(id);
}

/// The attempt orders only one holder's claims: another holder is `Busy`
/// whatever its attempt, and a take over still evicts the writer.
#[tokio::test]
async fn another_holder_is_busy_and_a_take_over_still_evicts() {
    let (manager, id, writer) = held_session("tab-A", Some(5));
    assert!(matches!(
        manager.attach_as(id, false, Some("tab-B"), Some(100), None),
        Err(AttachError::Busy)
    ));
    let taker = manager
        .attach_as(id, true, Some("tab-B"), Some(1), None)
        .expect("a take over by another holder evicts the writer");
    assert!(taker.writer);
    assert_eq!(writer.evict.reason(), Some(EndReason::TakenOver));
    manager.close(id);
}

/// When the writer or the claim has no attempt (a page of an older build),
/// the holder rule decides alone, as before the attempt existed.
#[tokio::test]
async fn without_an_attempt_on_either_side_the_holder_reclaims() {
    let (manager, id, writer) = held_session("tab-A", None);
    let claimed = manager
        .attach_as(id, false, Some("tab-A"), Some(1), None)
        .expect("a writer with no attempt is reclaimed by its holder");
    assert_eq!(writer.evict.reason(), Some(EndReason::TakenOver));
    manager
        .attach_as(id, false, Some("tab-A"), None, None)
        .expect("a claim with no attempt reclaims a writer that has one");
    assert_eq!(claimed.evict.reason(), Some(EndReason::TakenOver));
    manager.close(id);
}

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
/// end of the tail.
#[test]
fn a_resume_while_publishing_gets_every_byte_after_since_once() {
    // A round where the scheduler ran every attach before or after the pump
    // proves nothing, so it is run again.
    let rounds = (0..5)
        .take_while(|_| resume_while_publishing() == 0)
        .count();
    assert!(
        rounds < 5,
        "no resume happened while the pump ran, in 5 rounds"
    );
}

/// One round of the test above. Returns how many attaches got a tail with
/// some, but not all, of the lines published after `since`.
fn resume_while_publishing() -> usize {
    const LINES: usize = 800;
    let line = |i: usize| format!("L{i:04} {}\r\n", "-".repeat(70)).into_bytes();
    let kept = Arc::new(Mutex::new(KeptOutput::new(1, 24, 80)));
    let (tx, _) = broadcast::channel(BROADCAST_CAP);
    for i in 0..10 {
        publish(&kept, &tx, line(i));
    }
    let since = replay_and_subscribe(&kept, &tx, None).offset;
    let start = Arc::new(std::sync::Barrier::new(2));
    let pump = std::thread::spawn({
        let (kept, tx, start) = (kept.clone(), tx.clone(), start.clone());
        move || {
            start.wait();
            for i in 10..LINES {
                publish(&kept, &tx, line(i));
            }
        }
    });
    start.wait();
    let mut attaches = Vec::new();
    while !pump.is_finished() && attaches.len() < 200 {
        attaches.push(replay_and_subscribe(&kept, &tx, Some(since)));
    }
    pump.join().expect("the pump thread does not panic");

    let all: Vec<u8> = (0..LINES).flat_map(line).collect();
    let expected = &all[usize::try_from(since).expect("a small count")..];
    let mut during = 0;
    for (n, mut sub) in attaches.into_iter().enumerate() {
        assert!(sub.resume, "attach {n} resumes");
        assert_eq!(sub.offset, since + sub.replay.len() as u64, "attach {n}");
        if (1..expected.len()).contains(&sub.replay.len()) {
            during += 1;
        }
        let mut seen = sub.replay;
        while let Ok(chunk) = sub.rx.try_recv() {
            seen.extend(chunk);
        }
        assert_eq!(seen, expected, "attach {n}");
    }
    during
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
