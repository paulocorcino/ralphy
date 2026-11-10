use super::*;
use crate::runstate::UsageLite;
use ralphy_run_snapshot::{snapshot_path, SnapshotGuard};

fn test_ctx(runid: &str) -> SnapshotCtx {
    SnapshotCtx {
        runid: runid.into(),
        pid: std::process::id(),
        repo: "owner/repo".into(),
        branch: "afk/run-1".into(),
        started_at: "2026-07-24T10:00:00-03:00".into(),
        plan_path: ".ralphy/plan.md".into(),
    }
}

/// A document in a phase, for the anchor tests.
fn doc_in(state: &str, active: Option<u64>, since: Option<&str>) -> RunSnapshot {
    let mut doc = RunSnapshot {
        runid: "01ANCHOR".into(),
        ..RunSnapshot::default()
    };
    doc.phase.state = state.into();
    doc.phase.active = active;
    doc.phase.since = since.map(str::to_string);
    doc
}

#[test]
fn the_phase_anchor_is_minted_when_there_is_no_previous_document() {
    let mut next = doc_in("planning", Some(71), None);
    carry_phase_since(None, &mut next, || "MINTED".into());
    assert_eq!(
        next.phase.since.as_deref(),
        Some("MINTED"),
        "the first document of a run has no anchor to carry"
    );
}

/// The load-bearing one: an unchanged phase must not re-read the clock, or the
/// 250 ms tick would rewrite the document forever and the panel's clock would
/// restart on every write.
#[test]
fn an_unchanged_phase_carries_its_anchor_and_stays_byte_identical() {
    let prev = doc_in("executing", Some(71), Some("2026-07-29T10:00:00-03:00"));
    let mut next = doc_in("executing", Some(71), None);
    carry_phase_since(Some(&prev), &mut next, || {
        panic!("the clock must not be read when the phase is unchanged")
    });
    assert_eq!(next.phase.since, prev.phase.since);
    assert_eq!(
        next, prev,
        "an unchanged projection stays an unchanged write"
    );
}

#[test]
fn a_new_phase_state_restamps_the_anchor() {
    let prev = doc_in("planning", Some(71), Some("OLD"));
    let mut next = doc_in("executing", Some(71), None);
    carry_phase_since(Some(&prev), &mut next, || "NEW".into());
    assert_eq!(next.phase.since.as_deref(), Some("NEW"));
}

/// The same phase WORD on another issue is another activity: #72's execute
/// starts at zero, it does not inherit #71's clock.
#[test]
fn the_same_phase_on_a_new_issue_restamps_the_anchor() {
    let prev = doc_in("executing", Some(71), Some("OLD"));
    let mut next = doc_in("executing", Some(72), None);
    carry_phase_since(Some(&prev), &mut next, || "NEW".into());
    assert_eq!(next.phase.since.as_deref(), Some("NEW"));
}

/// The whole spine, end to end: events buffered on the ring BEFORE the worker
/// exists (the `deliver_from_pre_start_ring` pattern), the REAL delivery
/// worker driving the real engine over a tempdir repo, then the RAII guard.
#[test]
fn spine_writes_then_removes_the_document() {
    let dir = tempfile::tempdir().unwrap();
    let runid = "01SPINETESTRUNID";
    let queue = Arc::new(EventQueue::new());
    queue.push(RunEvent::QueueBuilt {
        count: 3,
        order: vec![71, 72, 73],
        stop_before: None,
        issues: serde_json::json!([
            {"number": 71, "title": "seventy-one"},
            {"number": 72, "title": "seventy-two"},
            {"number": 73, "title": "seventy-three"},
        ]),
        assignee_filter: None,
        scope: None,
    });
    queue.push(RunEvent::IssueStarted {
        number: 71,
        title: "seventy-one".into(),
    });
    queue.push(RunEvent::Executing {
        number: 71,
        budget_min: 45,
        model: "opus".into(),
        effort: None,
    });

    let handle = try_start_snapshot(
        test_ctx(runid),
        RunState::new("spine", 3),
        dir.path().to_path_buf(),
        queue,
        dir.path().join(".ralphy").join("plan.md"),
    )
    .expect("the snapshot worker spawns");
    handle.shutdown();

    let path = snapshot_path(dir.path(), runid);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(doc["phase"]["state"], "executing");
    assert_eq!(doc["phase"]["active"], 71);
    assert_eq!(doc["queue"]["total"], 3);
    assert_eq!(doc["issues"].as_array().unwrap().len(), 3);
    assert_eq!(doc["runid"], runid);

    drop(SnapshotGuard::new(dir.path(), runid));
    assert!(!path.exists(), "the guard removes the document at exit");
}

/// The end of a run: the real worker writes its last document, and the
/// run's guard moves it into the run folder, so the folder keeps each
/// issue's terminal status and the live document is gone.
#[test]
fn a_finished_run_leaves_its_final_snapshot_in_its_folder() {
    let dir = tempfile::tempdir().unwrap();
    let runid = "01FINALSNAPSHOT";
    let run_dir = dir
        .path()
        .join(".ralphy")
        .join("runs")
        .join("20261028-100000");
    std::fs::create_dir_all(&run_dir).unwrap();
    let queue = Arc::new(EventQueue::new());
    queue.push(RunEvent::QueueBuilt {
        count: 2,
        order: vec![71, 72],
        stop_before: None,
        issues: serde_json::json!([{"number": 71, "title": "a"}, {"number": 72, "title": "b"}]),
        assignee_filter: None,
        scope: None,
    });
    for number in [71, 72] {
        queue.push(RunEvent::IssueStarted {
            number,
            title: "t".into(),
        });
    }
    queue.push(RunEvent::IssueClosed {
        number: 71,
        tokens: 0,
        invocations: 1,
        usage: UsageLite::default(),
    });
    queue.push(RunEvent::NonGreen {
        number: 72,
        outcome: "Stuck".into(),
    });
    let guard = snapshot_guard(dir.path(), runid, &run_dir);
    try_start_snapshot(
        test_ctx(runid),
        RunState::new("final", 2),
        dir.path().to_path_buf(),
        queue,
        dir.path().join(".ralphy").join("plan.md"),
    )
    .expect("the snapshot worker spawns")
    .shutdown();
    drop(guard);

    assert!(
        !snapshot_path(dir.path(), runid).exists(),
        "the live document is gone"
    );
    let kept = ralphy_run_snapshot::read_final(&run_dir).expect("the run folder keeps it");
    assert_eq!(kept.runid, runid);
    let statuses: Vec<(u64, &str)> = kept
        .issues
        .iter()
        .map(|i| (i.number, i.status.as_str()))
        .collect();
    assert_eq!(statuses, [(71, "done"), (72, "non_green")]);
}

/// An engine over a tempdir repo whose `.ralphy/plan.md` holds `md`.
fn engine_over(dir: &std::path::Path, runid: &str, md: &str) -> (SnapshotEngine, PathBuf) {
    let plan_abs = dir.join(".ralphy").join("plan.md");
    std::fs::create_dir_all(plan_abs.parent().unwrap()).unwrap();
    std::fs::write(&plan_abs, md).unwrap();
    let engine = SnapshotEngine {
        ctx: test_ctx(runid),
        state: RunState::new("t", 1),
        last: None,
        repo_root: dir.to_path_buf(),
        warned: AtomicBool::new(false),
        plan: PlanProgress::default(),
        watch: PlanFileWatch::default(),
        plan_abs: plan_abs.clone(),
        stop_path: ralphy_run_snapshot::stop_path(dir, runid),
        stop_seen: false,
    };
    (engine, plan_abs)
}

fn written(dir: &std::path::Path, runid: &str) -> serde_json::Value {
    let path = snapshot_path(dir, runid);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn plan_written(number: u64, md: &str) -> RunEvent {
    RunEvent::PlanWritten {
        number,
        open_steps: 2,
        usage: UsageLite::default(),
        steps: plan_progress::parse_steps(md)
            .into_iter()
            .map(|s| (s.text, s.status.wire().to_string()))
            .collect(),
    }
}

/// The plan block is state, not a feed: a checkbox flipped on disk — with NO
/// transport, NO `EventCtx` and no events config anywhere — rewrites the
/// document the panel reads.
#[test]
fn checkbox_transition_rewrites_the_document() {
    let dir = tempfile::tempdir().unwrap();
    let runid = "01CHECKBOX";
    let md = "## Steps\n- [ ] first step\n- [ ] second step\n";
    let (mut engine, plan_abs) = engine_over(dir.path(), runid, md);

    engine.on_event(RunEvent::IssueStarted {
        number: 71,
        title: "seventy-one".into(),
    });
    engine.on_event(plan_written(71, md));
    engine.on_tick(true);

    let doc = written(dir.path(), runid);
    assert_eq!(doc["plan"]["issue"], 71);
    assert_eq!(doc["plan"]["steps"][0]["text"], "first step");
    assert_eq!(doc["plan"]["steps"][0]["status"], "open");
    assert_eq!(doc["plan"]["steps"][1]["status"], "open");

    std::fs::write(&plan_abs, "## Steps\n- [x] first step\n- [ ] second step\n").unwrap();
    plan_progress::testutil::advance_mtime(&plan_abs);
    engine.on_tick(false);

    let doc = written(dir.path(), runid);
    assert_eq!(
        doc["plan"]["steps"][0]["status"], "checked",
        "the poll carries the flipped checkbox into the document"
    );
    assert_eq!(doc["plan"]["steps"][1]["status"], "open");
}

/// The write budget (ADR-0047 §8) survives the poll: only a CHANGED plan file
/// causes a write.
#[test]
fn idle_tick_after_an_unchanged_poll_costs_no_write() {
    let dir = tempfile::tempdir().unwrap();
    let runid = "01IDLEPOLL";
    let md = "## Steps\n- [ ] first step\n";
    let (mut engine, plan_abs) = engine_over(dir.path(), runid, md);
    engine.on_event(RunEvent::IssueStarted {
        number: 71,
        title: "seventy-one".into(),
    });
    engine.on_event(plan_written(71, md));
    engine.on_tick(true);

    let path = snapshot_path(dir.path(), runid);
    std::fs::remove_file(&path).unwrap();
    engine.on_tick(false);
    assert!(
        !path.exists(),
        "an unchanged plan file costs no write, even with the poll armed"
    );

    std::fs::write(&plan_abs, "## Steps\n- [x] first step\n").unwrap();
    plan_progress::testutil::advance_mtime(&plan_abs);
    engine.on_tick(false);
    assert!(
        path.exists(),
        "a flipped checkbox does rewrite the document"
    );
}

/// The disarm oracle: `plan.md` on disk still holds #71's checked plan when
/// #72 starts, and it must NOT be published as #72's progress.
#[test]
fn a_new_issue_clears_the_previous_plan() {
    let dir = tempfile::tempdir().unwrap();
    let runid = "01DISARM";
    let md = "## Steps\n- [x] first step\n";
    let (mut engine, _plan_abs) = engine_over(dir.path(), runid, md);
    engine.on_event(RunEvent::IssueStarted {
        number: 71,
        title: "seventy-one".into(),
    });
    engine.on_event(plan_written(71, md));
    engine.on_tick(true);
    assert_eq!(written(dir.path(), runid)["plan"]["issue"], 71);

    engine.on_event(RunEvent::IssueStarted {
        number: 72,
        title: "seventy-two".into(),
    });
    engine.on_tick(true);
    let doc = written(dir.path(), runid);
    assert!(
        doc["plan"]["issue"].is_null(),
        "the previous issue's plan is disarmed: {}",
        doc["plan"]
    );
    assert_eq!(doc["plan"]["steps"].as_array().unwrap().len(), 0);
}

/// `ralphy-cli`'s tests share one process under `cargo test` AND run on
/// several threads, so two tests touching the process-global stop flag would
/// race each other. Same idiom, and same reason, as `ledger.rs`'s
/// `RALPHY_USAGE_DIR` lock.
static STOP_FLAG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Holds the lock for the test and restores `ralphy_core::stop` on the way
/// out, INCLUDING through a panicking assertion — a leaked `true` would ride
/// along into every test that runs after it (docs/adr/0054).
struct StopFlagGuard(
    #[expect(dead_code, reason = "the guard is held only for its Drop")]
    std::sync::MutexGuard<'static, ()>,
);

impl StopFlagGuard {
    fn acquire() -> Self {
        // A test that panicked while holding the lock poisons it; recover
        // rather than cascade, since the flag is reset right below anyway.
        let lock = STOP_FLAG_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ralphy_core::stop::clear();
        Self(lock)
    }
}

impl Drop for StopFlagGuard {
    fn drop(&mut self) {
        ralphy_core::stop::clear();
    }
}

/// The tick is the SETTER of the cooperative stop. Both halves matter: the
/// sentinel raises the flag, and the latch means the run pays no `exists()`
/// syscall for the rest of its life once it has.
#[test]
fn on_tick_raises_the_stop_flag_once_and_then_latches() {
    let _guard = StopFlagGuard::acquire();
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, _plan) = engine_over(dir.path(), "01STOPTICK", "");

    engine.on_tick(false);
    assert!(!engine.stop_seen, "no sentinel, no stop");
    assert!(!ralphy_core::stop::requested());

    std::fs::create_dir_all(ralphy_run_snapshot::snapshot_dir(dir.path())).unwrap();
    std::fs::write(
        ralphy_run_snapshot::stop_path(dir.path(), "01STOPTICK"),
        "{}",
    )
    .unwrap();
    engine.on_tick(false);
    assert!(engine.stop_seen, "the sentinel latches the engine");
    assert!(ralphy_core::stop::requested(), "and raises the flag");

    // Remove the sentinel: a latched engine must not consult it again, so
    // the flag stays raised. (A tick that re-derived the flag from the file
    // would silently un-stop a run whose sentinel was cleaned up mid-flight.)
    std::fs::remove_file(ralphy_run_snapshot::stop_path(dir.path(), "01STOPTICK")).unwrap();
    engine.on_tick(false);
    assert!(ralphy_core::stop::requested(), "a stop is not retractable");
}

/// The negative control: an engine whose sentinel never appears must leave
/// the flag alone for the whole run. Without this, an `exists()` inverted by
/// a refactor would stop every run at its first tick and this file would
/// still be green.
#[test]
fn ticks_without_a_sentinel_never_raise_the_flag() {
    let _guard = StopFlagGuard::acquire();
    let dir = tempfile::tempdir().unwrap();
    let (mut engine, _plan) = engine_over(dir.path(), "01NOSTOP", "");
    for _ in 0..5 {
        engine.on_tick(false);
    }
    assert!(!ralphy_core::stop::requested());
    assert!(!engine.stop_seen);
}

#[test]
fn engine_rewrites_only_when_the_projection_changes() {
    let dir = tempfile::tempdir().unwrap();
    let runid = "01UNCHANGED";
    let mut engine = SnapshotEngine {
        ctx: test_ctx(runid),
        state: RunState::new("t", 1),
        last: None,
        repo_root: dir.path().to_path_buf(),
        warned: AtomicBool::new(false),
        plan: PlanProgress::default(),
        watch: PlanFileWatch::default(),
        plan_abs: dir.path().join(".ralphy").join("plan.md"),
        stop_path: ralphy_run_snapshot::stop_path(dir.path(), runid),
        stop_seen: false,
    };
    engine.on_start();
    let path = snapshot_path(dir.path(), runid);
    assert!(path.exists(), "on_start publishes before any event");

    // An idle tick must not rewrite: delete the file and prove the unchanged
    // tick leaves it deleted (a rewrite would recreate it).
    std::fs::remove_file(&path).unwrap();
    engine.on_tick(false);
    assert!(!path.exists(), "an unchanged fold costs no write");

    // A folded event changes the projection, so the next tick does write.
    engine.on_event(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    });
    engine.on_tick(true);
    assert!(path.exists(), "a changed fold rewrites the document");
}
