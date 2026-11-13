//! A planner reaped by the operator's stop returns its usage. This is its own
//! test binary: the stop flag is process-global, so no other test may run
//! beside it (the rule of `crates/ralphy-core/tests/stop/main.rs`).
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ralphy_core::{Agent, PlanStopped};

mod common;

use common::{issue, Setup};

/// The conversation id of the fake (`CONVERSATION` in the child).
const FAKE_CONVERSATION: &str = "00000000-0000-4000-8000-0000000000aa";

/// How long the stop may take to end `plan()`.
const STOP_GRACE: Duration = Duration::from_secs(10);

static STOP_FLAG_LOCK: Mutex<()> = Mutex::new(());

/// Holds the lock of the stop flag, and clears the flag at both ends.
struct StopFlagGuard(
    #[expect(
        dead_code,
        reason = "the guard holds the lock for its drop and is never read"
    )]
    MutexGuard<'static, ()>,
);

impl StopFlagGuard {
    fn acquire() -> Self {
        // A panicking test poisons the lock; the flag is cleared right below.
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

/// Plan against the `interrupted` fake and stop the run once the fake holds
/// the task. Returns the error of `plan()` and the time from the stop to it.
fn plan_and_stop() -> (anyhow::Error, Duration) {
    let _flag = StopFlagGuard::acquire();
    let setup = Setup::new("interrupted");
    std::thread::scope(|scope| {
        let stopper = scope.spawn(|| {
            let give_up = Instant::now() + Duration::from_secs(30);
            while Instant::now() < give_up && !task_is_held(&setup) {
                std::thread::sleep(Duration::from_millis(50));
            }
            // The fake writes its task step right after it reads the task.
            std::thread::sleep(Duration::from_millis(300));
            ralphy_core::stop::request();
            Instant::now()
        });
        let result = setup.agent(None).plan(&issue(), &setup.ws());
        let ended = Instant::now();
        let requested = stopper.join().expect("the stopper thread ends");
        let err = result.expect_err("a stopped plan has no plan");
        (err, ended.saturating_duration_since(requested))
    })
}

/// The order log shows the task line, after the proof turn.
fn task_is_held(setup: &Setup) -> bool {
    setup.order().len() >= 3
}

fn stopped(err: &anyhow::Error) -> &PlanStopped {
    err.downcast_ref::<PlanStopped>()
        .expect("the error of a stopped plan is a PlanStopped")
}

#[test]
fn a_stopped_plan_returns_its_usage() {
    let (err, _) = plan_and_stop();
    assert_eq!(stopped(&err).usage.input, 300);
}

#[test]
fn a_stopped_plan_carries_its_conversation_id() {
    let (err, _) = plan_and_stop();
    assert_eq!(stopped(&err).session_id.as_deref(), Some(FAKE_CONVERSATION));
}

#[test]
fn a_stopped_plan_ends_within_the_stop_grace() {
    let (_, took) = plan_and_stop();
    assert!(took < STOP_GRACE, "plan() took {took:?} after the stop");
}
