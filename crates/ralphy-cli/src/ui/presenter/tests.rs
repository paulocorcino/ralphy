use super::*;
use crate::runstate::{IssueStatus, UsageLite};

/// A plain (colour off, so no `indicatif` I/O) renderer over a fresh live state —
/// the seam that lets `drive` be driven directly.
fn plain_renderer() -> (Renderer, LiveState) {
    let renderer = Renderer {
        multi: MultiProgress::new(),
        state: Arc::new(Mutex::new(LiveState::default())),
        opts: RenderOpts {
            color: false,
            emoji: true,
        },
        price: PriceTable::load(),
    };
    (renderer, LiveState::default())
}

fn usage(input: u64, output: u64) -> UsageLite {
    UsageLite {
        input,
        cache_read: 0,
        cache_creation: 0,
        output,
        model: Some("claude-opus-4".into()),
    }
}

/// `drive` must derive the `done` line's whole tail from the fold: the elapsed
/// clock from its own anchor, the model/effort from the folded entry, and the
/// meter from the stashed PLAN usage plus this execution usage. This is the
/// contract the deleted `ActiveIssue` used to carry, and nothing else pins it.
#[test]
fn drive_derives_the_done_line_extra_from_the_fold() {
    let (r, mut s) = plain_renderer();
    r.drive(
        &mut s,
        &RunEvent::IssueStarted {
            number: 7,
            title: "t".into(),
        },
    );
    r.drive(
        &mut s,
        &RunEvent::PlanWritten {
            number: 7,
            open_steps: 3,
            usage: usage(100, 10),
            steps: vec![],
        },
    );
    r.drive(
        &mut s,
        &RunEvent::Executing {
            number: 0,
            budget_min: 45,
            model: "claude-opus-4".into(),
            effort: Some("medium".into()),
        },
    );
    let extra = r.drive(
        &mut s,
        &RunEvent::IssueClosed {
            number: 7,
            tokens: 0,
            invocations: 0,
            usage: usage(200, 20),
        },
    );
    assert_eq!(extra.model.as_deref(), Some("claude-opus-4"));
    assert_eq!(extra.effort.as_deref(), Some("medium"));
    assert!(extra.duration.is_some(), "the anchor keys issue 7");
    let meter = extra.meter.expect("a plan+exec meter");
    assert_eq!(meter.usage.input, 300, "plan (100) + exec (200) input");
    assert_eq!(meter.usage.output, 30, "plan (10) + exec (20) output");
    // The issue is closed: the anchor is dropped so no later line reuses it.
    assert!(s.active_start.is_none());
}

/// A `done` for an issue that is not the active one carries no derived tail —
/// the old `filter(|a| a.number == *number)` guard, kept.
#[test]
fn drive_done_for_a_foreign_issue_carries_no_derived_tail() {
    let (r, mut s) = plain_renderer();
    r.drive(
        &mut s,
        &RunEvent::IssueStarted {
            number: 7,
            title: "t".into(),
        },
    );
    let extra = r.drive(
        &mut s,
        &RunEvent::IssueClosed {
            number: 99,
            tokens: 0,
            invocations: 0,
            usage: UsageLite::default(),
        },
    );
    assert!(extra.duration.is_none());
    assert!(extra.model.is_none());
    assert!(extra.meter.is_none());
}

/// A zero-step plan is terminal on arrival: `drive` must settle the active line
/// then and there, or the per-second ticker repaints a frozen clock for an issue
/// the run has already left behind.
#[test]
fn drive_settles_the_active_line_on_a_terminal_plan() {
    let (r, mut s) = plain_renderer();
    r.drive(
        &mut s,
        &RunEvent::IssueStarted {
            number: 7,
            title: "t".into(),
        },
    );
    assert!(s.active_start.is_some(), "the anchor is set while planning");
    r.drive(
        &mut s,
        &RunEvent::PlanWritten {
            number: 7,
            open_steps: 0,
            usage: UsageLite::default(),
            steps: vec![],
        },
    );
    assert_eq!(s.run.issues[0].status, IssueStatus::Infeasible);
    assert!(
        s.active_start.is_none(),
        "an infeasible plan takes the active line down"
    );

    // A bundle verdict is the same shape and must settle too.
    let (r2, mut s2) = plain_renderer();
    r2.drive(
        &mut s2,
        &RunEvent::IssueStarted {
            number: 8,
            title: "b".into(),
        },
    );
    r2.drive(&mut s2, &RunEvent::NeedsSplit { number: 8 });
    assert_eq!(s2.run.issues[0].status, IssueStatus::NeedsSplit);
    assert!(s2.active_start.is_none());
}

/// A usage-limit sleep keeps the issue's wall clock running (the old presenter
/// kept `ActiveIssue` across the sleep) — only the spinner comes down.
#[test]
fn drive_sleep_keeps_the_wall_clock_anchor() {
    let (r, mut s) = plain_renderer();
    r.drive(
        &mut s,
        &RunEvent::IssueStarted {
            number: 7,
            title: "t".into(),
        },
    );
    r.drive(
        &mut s,
        &RunEvent::SleepStarted {
            reset: "14:30".into(),
            target_epoch: 1_700_000_000,
        },
    );
    assert_eq!(s.active_start.map(|(n, _)| n), Some(7));
    r.drive(&mut s, &RunEvent::SleepEnded);
    assert_eq!(s.active_start.map(|(n, _)| n), Some(7));
}

/// The enqueue that `on_event` performs must never block the run thread, even if
/// the render thread is wedged in a stalled console write and drains nothing — the
/// freeze this whole design fixes. An unconsumed channel models the wedged renderer.
#[test]
fn enqueue_is_off_the_run_path_even_with_a_stalled_renderer() {
    let (tx, _rx) = mpsc::channel::<PresenterMsg>();
    let start = Instant::now();
    for _ in 0..1000 {
        tx.send(PresenterMsg::Event(RunEvent::SleepEnded))
            .expect("send never blocks");
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(50),
        "the on_event enqueue must be off the run path, took {elapsed:?}"
    );
}

/// Teardown must not hang if the render thread holds the state lock (wedged mid-draw
/// in a stalled write): the bounded acquire returns `None` after its budget.
#[test]
fn finalize_lock_is_bounded_when_state_is_held() {
    let handle = PresenterHandle {
        multi: MultiProgress::new(),
        state: Arc::new(Mutex::new(LiveState::default())),
        color: true,
        flush: None,
        edge: Arc::new(Mutex::new(crate::ui::EdgeNoticeState::default())),
    };
    let _held = handle.state.lock().expect("hold the state lock");
    let start = Instant::now();
    let got = handle.lock_state_bounded(Duration::from_millis(100));
    let elapsed = start.elapsed();
    assert!(got.is_none(), "a held lock must not be acquired");
    assert!(
        elapsed < Duration::from_secs(1),
        "the bounded acquire must give up near its budget, took {elapsed:?}"
    );
}
