//! The watch bell: when a poll or a watch change rings it.

use super::*;

/// A folder expanded while a poll is in flight has to reach the peer NOW: the
/// peer only learns of a dir a request named, so without the bell the browser
/// opened a project blind for a whole 25 s window — the burst of `watch`
/// frames lands after the first poll is already out.
#[tokio::test]
async fn a_watch_added_after_the_first_poll_rings_the_bell() {
    let set = PeerWatchSet::new("", false);
    let bell = set.changed.clone();
    let waiting = tokio::spawn(async move { bell.notified().await });
    tokio::task::yield_now().await;

    set.add("docs/agents", false);
    tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .expect("adding a watched dir must wake the poll in flight")
        .unwrap();
    assert!(set.snapshot().contains(&"docs/agents".to_string()));
    assert!(!set.runs());
}

/// A repeated `watch` for a dir already held is a no-op, and must not cost a
/// re-post: a browser re-sending its set would otherwise restart every poll.
#[tokio::test]
async fn re_adding_a_held_dir_does_not_ring_the_bell() {
    let set = PeerWatchSet::new("docs", false);
    set.add("docs", false);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), set.changed.notified())
            .await
            .is_err(),
        "an idempotent watch must not restart the poll in flight"
    );
}

/// The poll deadline has to outlast the window the peer was ASKED to hold,
/// with room for the peer's own work either side of it. When it did not, every
/// poll timed out 800 ms before the answer arrived and the browser stopped
/// receiving `tree.dirty` altogether.
#[test]
fn a_poll_deadline_outlasts_the_window_it_asks_for() {
    let window = Duration::from_millis(PEER_POLL_WINDOW_MS);
    assert!(
        PEER_POLL_DEADLINE >= window + Duration::from_secs(10),
        "a {PEER_POLL_DEADLINE:?} deadline leaves too little over a {window:?} window"
    );
}

/// Only a poll that actually reported a change may be followed by an instant
/// re-post. Every other outcome — including a 200 nobody can read — has to
/// wait, because each poll costs one TCP connection and one ephemeral port on
/// the polling host (the 2026-09-01 exhaustion).
#[test]
fn only_a_changed_poll_re_posts_at_once() {
    assert_eq!(next_poll_delay(PollCycle::Changed), Duration::ZERO);
    assert_eq!(next_poll_delay(PollCycle::Quiet), PEER_POLL_QUIET_PAUSE);
    assert_eq!(next_poll_delay(PollCycle::Unreadable), PEER_POLL_BACKOFF);
    assert_eq!(next_poll_delay(PollCycle::Failed), PEER_POLL_BACKOFF);
    for cycle in [PollCycle::Quiet, PollCycle::Unreadable, PollCycle::Failed] {
        assert!(
            !next_poll_delay(cycle).is_zero(),
            "{cycle:?} must not re-post at once"
        );
    }
}

/// A peer that was down answers its first poll from an EMPTY subscription, so
/// only the poller knows the watch set must be re-announced — once, on the first
/// readable answer after a failed one.
#[test]
fn a_readable_poll_after_a_failed_one_owes_the_watch_set_once() {
    let mut up = CatchUp::default();
    assert!(!up.after(PollCycle::Quiet), "no outage, nothing owed");
    assert!(!up.after(PollCycle::Changed));

    let mut after_fail = CatchUp::default();
    assert!(!after_fail.after(PollCycle::Failed));
    assert!(!after_fail.after(PollCycle::Failed));
    assert!(after_fail.after(PollCycle::Quiet), "the peer is back");
    assert!(
        !after_fail.after(PollCycle::Quiet),
        "owed once, not on every poll"
    );

    let mut after_unreadable = CatchUp::default();
    assert!(!after_unreadable.after(PollCycle::Unreadable));
    assert!(after_unreadable.after(PollCycle::Changed));

    // A folder expanded while the peer is down restarts the poll; that must not
    // cancel what the outage owes.
    let mut restarted = CatchUp::default();
    assert!(!restarted.after(PollCycle::Failed));
    assert!(!restarted.after(PollCycle::Restarted));
    assert!(restarted.after(PollCycle::Quiet));
}
