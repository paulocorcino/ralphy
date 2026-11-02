use super::*;

#[test]
fn the_idle_watchdog_reaps_a_silent_child_long_before_the_wall_timeout() {
    // This is the OpenCode failure mode in miniature: a child that hit a provider
    // quota block, swallowed it into a silent retry, and now prints nothing at all.
    // No stderr matcher can see a failure that is never printed — but the silence
    // itself is unmistakable, and it is the only signal that catches this.
    let log_path = temp_log("idle-kill");
    remove_scratch(&log_path);

    let started = Instant::now();
    let r = HeadlessCall::new(
        child_cmd("sleep"),
        "ignored prompt",
        // A wall timeout the child would happily sleep out: a prompt return can
        // only mean the idle watchdog fired.
        Duration::from_secs(60),
        &log_path,
    )
    .idle_window(Duration::from_secs(1))
    .run()
    .expect("an idle-watched run should not error");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "the watchdog reaped the silent child, it did not wait out the 60s wall timeout \
         (took {elapsed:?})"
    );
    assert!(r.idle_killed, "the idle watchdog fired, not the wall clock");
    assert!(
        r.timed_out,
        "an idle kill still reports as a timeout so classification is unchanged"
    );
    assert!(!r.exited_cleanly, "a killed child did not exit cleanly");
    remove_scratch(&log_path);
}

#[test]
fn the_idle_watchdog_spares_a_child_that_keeps_talking() {
    // The regression this whole change risks: killing healthy work. The chatty
    // child runs far longer than the idle window but never goes quiet for a full
    // one, so it must survive — proving the watchdog measures silence, not elapsed
    // time. (That distinction is the entire reason it replaced a wall-clock cap.)
    let log_path = temp_log("idle-chatty");
    remove_scratch(&log_path);

    let r = HeadlessCall::new(
        child_cmd("chatty"),
        "ignored prompt",
        // Wall timeout well past the idle window: whatever ends this run, it is
        // not the wall clock racing the watchdog.
        Duration::from_secs(3),
        &log_path,
    )
    .idle_window(Duration::from_secs(1))
    .run()
    .expect("a chatty child should not error");

    assert!(
        !r.idle_killed,
        "a child emitting a line every 100ms is never idle for a 1s window"
    );
    assert!(
        r.log.contains("tick "),
        "the child's output was captured while it ran"
    );
    remove_scratch(&log_path);
}

#[test]
fn degraded_only_child_is_idle_reaped_despite_talking() {
    // The headless counterpart to the PTY `Waiting for API response` wedge: a child
    // that keeps printing degraded banners at the chatty cadence but makes no real
    // progress. Because every line matches the degraded predicate, none of them
    // rearm the idle beacon, so the watchdog reaps it despite the constant chatter.
    // This FAILS before the degraded plumbing exists (the banners would beat).
    let log_path = temp_log("degraded-idle");
    remove_scratch(&log_path);

    let started = Instant::now();
    let r = HeadlessCall::new(
        child_cmd("degraded-chatty"),
        "ignored prompt",
        Duration::from_secs(60),
        &log_path,
    )
    .degraded_line(|l| l.contains(DEGRADED_MARKER))
    .idle_window(Duration::from_secs(1))
    .run()
    .expect("a degraded-watched run should not error");
    let elapsed = started.elapsed();

    assert!(
        r.idle_killed,
        "a child emitting only degraded lines is reaped by the idle watchdog"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "reaped promptly despite talking every 100ms (took {elapsed:?})"
    );
    remove_scratch(&log_path);
}

#[test]
fn healthy_child_with_a_degraded_matcher_survives() {
    // The non-regression: arming a degraded matcher must not endanger a healthy
    // child. The chatty child's lines never match the predicate, so they still beat
    // the idle beacon and the child survives — proving the degraded gate only ever
    // stops *matching* lines from counting as progress.
    let log_path = temp_log("degraded-healthy");
    remove_scratch(&log_path);

    let r = HeadlessCall::new(
        child_cmd("chatty"),
        "ignored prompt",
        Duration::from_secs(3),
        &log_path,
    )
    .degraded_line(|l| l.contains(DEGRADED_MARKER))
    .idle_window(Duration::from_secs(1))
    .run()
    .expect("a healthy chatty child should not error");

    assert!(
        !r.idle_killed,
        "the matcher never matches, so healthy lines still rearm the beacon"
    );
    assert!(
        r.log.contains("tick "),
        "the child's healthy output was captured while it ran"
    );
    remove_scratch(&log_path);
}

// ── process-tree teardown: no descendant survives a kill ────────────────────

/// A unique temp path for a per-test heartbeat file, written by the LEAF of the
/// three-level helper tree.
fn temp_heartbeat(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "ralphy-headless-beat-{tag}-{}.txt",
        std::process::id()
    ))
}

/// Build the root of the three-level heartbeat tree, forwarding the leaf's
/// heartbeat path as the second argv word.
fn heartbeat_cmd(beat: &std::path::Path) -> Command {
    let mut cmd = child_cmd("heartbeat-tree");
    cmd.arg(beat);
    cmd
}

/// Assert the tree really ran and then really stopped: ALL THREE levels wrote
/// while it was alive (otherwise the whole test passes vacuously against a tree
/// that never got built), and the file's byte length is UNCHANGED after a further
/// 1.5 s — any surviving member would have appended ~15 more ticks in that window.
fn assert_heartbeat_grew_then_froze(beat: &std::path::Path) {
    let text = std::fs::read_to_string(beat).unwrap_or_default();
    for label in ["L1", "L2", "leaf"] {
        assert!(
            text.contains(&format!("{label} tick")),
            "level {label:?} never wrote, so its freeze proves nothing — got {text:?}"
        );
    }
    let len_at_kill = text.len() as u64;
    std::thread::sleep(Duration::from_millis(1500));
    let len_after = std::fs::metadata(beat).map(|m| m.len()).unwrap_or(0);
    assert_eq!(
        len_after, len_at_kill,
        "a member of the tree outlived the kill and kept writing"
    );
}

#[test]
fn a_wall_timeout_leaves_no_surviving_descendant() {
    // `timeout_with_surviving_grandchild_still_returns_promptly` only proves the
    // CALL returns — the reader reaching EOF says nothing about whether the
    // grandchild is still running. This is the missing half: the leaf's own writes
    // must stop, which is the only observation that outlives the closed pipes.
    let log_path = temp_log("beat-wall");
    let beat = temp_heartbeat("wall");
    remove_scratch(&log_path);
    remove_scratch(&beat);

    let r = HeadlessCall::new(
        heartbeat_cmd(&beat),
        "ignored prompt",
        // 2 s, not the tighter margin the tree would also survive: the kill lands
        // one 500 ms poll tick LATE, and everything before it must fit three
        // sequential process creations on a contended CI runner. The tree sleeps
        // 60 s either way, so a wider window costs the suite nothing.
        Duration::from_secs(2),
        &log_path,
    )
    .run()
    .expect("a wall-timed tree run should not error");

    assert!(r.timed_out, "the tree outlived its 2s wall timeout");
    assert_heartbeat_grew_then_froze(&beat);

    remove_scratch(&log_path);
    remove_scratch(&beat);
}

#[test]
fn an_idle_kill_leaves_no_surviving_descendant() {
    // The idle path tears the tree down through a different door than the wall
    // clock, so it needs its own proof. The leaf writes to a FILE, never to stdout,
    // so nothing it does rearms the idle beacon — that is what makes a tree which
    // is demonstrably alive still reachable by the watchdog.
    let log_path = temp_log("beat-idle");
    let beat = temp_heartbeat("idle");
    remove_scratch(&log_path);
    remove_scratch(&beat);

    let r = HeadlessCall::new(
        heartbeat_cmd(&beat),
        "ignored prompt",
        // A wall timeout the tree would happily sleep out: only the watchdog can
        // end this run.
        Duration::from_secs(60),
        &log_path,
    )
    .idle_window(Duration::from_secs(1))
    .run()
    .expect("an idle-watched tree run should not error");

    assert!(r.idle_killed, "the idle watchdog fired, not the wall clock");
    assert_heartbeat_grew_then_froze(&beat);

    remove_scratch(&log_path);
    remove_scratch(&beat);
}

#[test]
fn a_command_with_an_unpiped_stream_errors_and_leaves_no_child_running() {
    // The caller builds the stdio. A missing pipe is a run error, and the child
    // that was already spawned must not be left running.
    type Unpipe = fn(&mut Command);
    let cases: [(&str, Unpipe); 3] = [
        ("stdin", |c| {
            c.stdin(Stdio::inherit());
        }),
        ("stdout", |c| {
            c.stdout(Stdio::inherit());
        }),
        ("stderr", |c| {
            c.stderr(Stdio::inherit());
        }),
    ];
    for (tag, unpipe) in cases {
        let beat = temp_heartbeat(&format!("unpiped-{tag}"));
        remove_scratch(&beat);
        let mut cmd = child_cmd("heartbeat");
        cmd.arg(&beat);
        unpipe(&mut cmd);

        let err = run_headless(cmd, "ignored prompt", Duration::from_secs(30))
            .expect_err("a command with an unpiped stream is a run error");
        assert!(
            format!("{err:#}").contains("was not piped"),
            "the error names the missing pipe ({tag}) — got {err:#}"
        );

        std::thread::sleep(Duration::from_millis(500));
        let len_after_error = std::fs::metadata(&beat).map(|m| m.len()).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(1000));
        let len_later = std::fs::metadata(&beat).map(|m| m.len()).unwrap_or(0);
        assert_eq!(
            len_later, len_after_error,
            "the child spawned for the {tag} case kept running after the error"
        );
        remove_scratch(&beat);
    }
}

#[test]
fn a_disabled_idle_watchdog_leaves_the_run_exactly_as_it_was() {
    // `0` is the operator's opt-out: the silent child now runs to the wall timeout
    // exactly as it did before the watchdog existed.
    let log_path = temp_log("idle-off");
    remove_scratch(&log_path);

    let r = HeadlessCall::new(
        child_cmd("sleep"),
        "ignored prompt",
        Duration::from_millis(300),
        &log_path,
    )
    .idle_minutes(0)
    .run()
    .expect("a run with the watchdog disabled should not error");

    assert!(r.timed_out, "the wall timeout still fires");
    assert!(!r.idle_killed, "the watchdog was off, so it did not fire");
    remove_scratch(&log_path);
}
