use super::*;

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
/// evicts nobody. An equal or higher one reclaims the slot, the half-open
/// case of ADR-0051 §9.
#[tokio::test]
async fn a_lower_attempt_of_the_same_holder_is_superseded_and_evicts_nobody() {
    let (manager, id, writer) = held_session("tab-A", Some(5));
    assert!(
        matches!(
            manager.attach_as(id, false, Some("tab-A"), Some(4), None),
            Err(AttachError::Superseded)
        ),
        "attempt 4 after 5 is a late socket"
    );
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

/// A take over always wins, also as the slot's own holder with a lower
/// attempt: two windows that share a holder (a duplicated tab, a fence
/// popup) swap the console with Take over, and a page reconnect never sends
/// one.
#[tokio::test]
async fn a_take_over_with_a_lower_attempt_of_the_same_holder_evicts_the_writer() {
    let (manager, id, writer) = held_session("tab-A", Some(5));
    let taker = manager
        .attach_as(id, true, Some("tab-A"), Some(4), None)
        .expect("a take over is never a late socket");
    assert!(taker.writer);
    assert_eq!(writer.evict.reason(), Some(EndReason::TakenOver));
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
