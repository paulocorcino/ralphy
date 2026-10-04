use super::*;
use crate::desk::{DeskFence, DeskNote, DeskRect};

const NOW: i64 = 1_800_000_000_000;

fn rect(left: f64) -> DeskRect {
    DeskRect {
        left,
        top: 20.0,
        width: 640.0,
        height: 480.0,
    }
}

fn window(id: &str, left: f64, session: Option<u64>) -> DeskRecord {
    DeskRecord {
        id: id.into(),
        repo: "owner/repo".into(),
        agent: "claude".into(),
        kind: "agent".into(),
        rect: rect(left),
        session_id: session,
        ts: 1,
        ..Default::default()
    }
}

fn desk(windows: Vec<DeskRecord>) -> DeskStore {
    DeskStore {
        windows,
        ..Default::default()
    }
}

fn version(reason: Reason, started_at: i64, d: DeskStore) -> Version {
    Version {
        kind: VERSION_KIND.into(),
        id: started_at,
        started_at,
        saved_at: started_at,
        reason,
        desk: d,
    }
}

#[test]
fn a_reconnect_is_not_a_layout_change() {
    let held = desk(vec![window("w1", 100.0, Some(1))]);
    let mut after = held.clone();
    after.windows[0].ts = 99;
    after.windows[0].session_id = Some(2);
    after.windows[0].rect.left = 100.4;
    let newest = version(Reason::Change, NOW - 3_600_000, held.clone());
    assert_eq!(
        capture_step(Some(&newest), &held, &after, NOW),
        Step::Nothing
    );
}

#[test]
fn a_change_goes_into_the_newest_version_for_one_minute_from_its_first_change() {
    let held = desk(vec![window("w1", 100.0, Some(1))]);
    let moved = desk(vec![window("w1", 300.0, Some(1))]);
    let open = version(Reason::Change, NOW - 59_000, held.clone());
    assert_eq!(
        capture_step(Some(&open), &held, &moved, NOW),
        Step::Overwrite
    );
    let closed = version(Reason::Change, NOW - 61_000, held.clone());
    assert_eq!(
        capture_step(Some(&closed), &held, &moved, NOW),
        Step::Append { keep_before: false }
    );
}

#[test]
fn a_change_never_goes_into_the_version_saved_before_a_restore() {
    let held = desk(vec![window("w1", 100.0, Some(1))]);
    let moved = desk(vec![window("w1", 300.0, Some(1))]);
    let saved = version(Reason::BeforeRestore, NOW - 1_000, held.clone());
    assert_eq!(
        capture_step(Some(&saved), &held, &moved, NOW),
        Step::Append { keep_before: false }
    );
}

#[test]
fn a_change_within_a_minute_of_a_restore_goes_into_the_restore() {
    let restored = desk(vec![window("w1", 100.0, Some(1))]);
    let mut named = restored.clone();
    named.windows[0].console_name = Some("repo #1".into());
    let v = version(Reason::Upload, NOW - 2_000, restored.clone());
    assert_eq!(
        capture_step(Some(&v), &restored, &named, NOW),
        Step::Overwrite
    );
}

#[test]
fn the_first_change_keeps_the_desk_from_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let before = desk(vec![window("w1", 100.0, Some(1))]);
    let after = desk(vec![window("w1", 300.0, Some(1))]);
    capture(dir.path(), &before, &after, NOW).unwrap();
    let rows = list(dir.path()).unwrap();
    assert_eq!(rows.len(), 2, "the desk before the change and after it");
    let oldest = load(dir.path(), rows[1].id).unwrap();
    assert!(same_layout(&oldest.desk, &before));
    let newest = load(dir.path(), rows[0].id).unwrap();
    assert!(same_layout(&newest.desk, &after));
}

#[test]
fn an_overwrite_keeps_the_start_and_moves_the_save_time() {
    let dir = tempfile::tempdir().unwrap();
    let a = desk(vec![window("w1", 100.0, Some(1))]);
    let b = desk(vec![window("w1", 200.0, Some(1))]);
    let c = desk(vec![window("w1", 300.0, Some(1))]);
    append(dir.path(), &a, Reason::Change, NOW).unwrap();
    capture(dir.path(), &a, &b, NOW + 61_000).unwrap();
    capture(dir.path(), &b, &c, NOW + 90_000).unwrap();
    let rows = list(dir.path()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].started_at, NOW + 61_000);
    assert_eq!(rows[0].saved_at, NOW + 90_000);
    assert!(same_layout(&load(dir.path(), rows[0].id).unwrap().desk, &c));
}

#[test]
fn the_history_keeps_the_fifty_newest_versions() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..(HISTORY_MAX as i64 + 1) {
        let d = desk(vec![window("w1", i as f64, None)]);
        append(dir.path(), &d, Reason::Change, NOW + i).unwrap();
    }
    let rows = list(dir.path()).unwrap();
    assert_eq!(rows.len(), HISTORY_MAX);
    assert_eq!(rows.last().map(|r| r.id), Some(NOW + 1), "the oldest went");
}

#[test]
fn two_versions_in_the_same_ms_keep_their_order() {
    let dir = tempfile::tempdir().unwrap();
    let first = append(dir.path(), &desk(vec![]), Reason::BeforeRestore, NOW).unwrap();
    let second = append(dir.path(), &desk(vec![]), Reason::Restore, NOW).unwrap();
    assert!(second > first);
}

#[test]
fn a_damaged_version_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    append(dir.path(), &desk(vec![]), Reason::Change, NOW).unwrap();
    std::fs::write(dir.path().join(format!("{}.json", NOW + 5)), "{ not json").unwrap();
    let rows = list(dir.path()).unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![NOW]);
}

#[test]
fn an_unknown_version_reads_as_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let e = load(dir.path(), NOW).unwrap_err();
    assert!(is_not_found(&e));
}

#[test]
fn a_restore_never_ends_a_running_console_and_never_shows_one_twice() {
    // Current: `a` runs and is in the version; `r` runs and is not; `p` is a
    // placeholder that is not; `new` runs session 3, which the version saved
    // under the id `old`.
    let mut by_session = window("new", 500.0, Some(3));
    by_session.daemon_id = Some("D".into());
    let mut current = desk(vec![
        window("a", 10.0, Some(1)),
        window("r", 20.0, Some(2)),
        window("p", 30.0, None),
        by_session,
    ]);
    current
        .checkouts
        .insert("owner/repo".into(), "wt-now".into());
    current.generation = 7;
    let mut old = window("old", 900.0, Some(3));
    old.daemon_id = Some("D".into());
    let mut saved = desk(vec![window("a", 700.0, Some(42)), old]);
    saved.fences.push(DeskFence {
        id: "f1".into(),
        rect: rect(0.0),
        ..Default::default()
    });
    saved.notes.push(DeskNote {
        id: "n1".into(),
        rect: rect(5.0),
        ..Default::default()
    });
    saved
        .checkouts
        .insert("owner/repo".into(), "wt-then".into());

    let (out, running) = restore(current, saved, NOW);

    let ids: Vec<&str> = out.windows.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "new", "r"]);
    let a = &out.windows[0];
    assert_eq!(a.rect.left, 700.0, "the version's place");
    assert_eq!(a.session_id, Some(1), "the running session");
    let new = &out.windows[1];
    assert_eq!(new.rect.left, 900.0);
    assert_eq!(new.session_id, Some(3));
    assert!(out.windows.iter().take(2).all(|w| w.ts == NOW));
    assert_eq!(running, HashSet::from(["r".to_string()]));
    assert_eq!(out.fences.len(), 1);
    assert_eq!(out.fences[0].ts, NOW);
    assert_eq!(out.notes.len(), 1);
    assert_eq!(out.notes[0].ts, NOW);
    assert_eq!(
        out.checkouts.get("owner/repo").map(String::as_str),
        Some("wt-now")
    );
    assert_eq!(out.generation, 7, "the caller sets the generation");
}
