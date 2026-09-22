use super::*;
use crate::session::console_spec;

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

/// The writer slot and the watcher list are separate registers: a watcher may
/// never make the slot LOOK occupied (which would refuse an honest attach with
/// `409`), and it must be reachable while the slot IS held (which is the whole
/// point — a second workbench sees the session instead of stealing it).
///
/// Spawns the platform shell rather than the helper child bin: `CARGO_BIN_EXE_*`
/// is visible only to integration tests (CONTEXT.md → Testing conventions), and
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
            spec,
        )
        .expect("the platform shell must spawn — the free console depends on it");
    drop(writer); // the slot is free; only the watcher list is about to fill

    let watcher = manager
        .watch(id)
        .expect("a live session is always watchable");
    assert!(!watcher.writer, "watch() yields a reader, never the baton");
    let taker = manager
        .attach(id, false)
        .expect("a registered watcher must not make the writer slot look busy");
    assert!(taker.writer, "a plain attach takes the baton");
    let second = manager
        .watch(id)
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
