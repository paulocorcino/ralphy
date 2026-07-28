//! The plan-step poller (#96): diffs a plan's checkbox lines against its last-seen
//! snapshot and delivers a `dev.ralphy.plan.step` on each checked/noticed transition.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::delivery::{deliver, RETRY_BASE_BACKOFF};
use crate::events::client::EventSink;
use crate::events::envelope::EventCtx;
use crate::plan_progress::{self, PlanFileWatch, PlanStep, StepStatus};
use crate::runstate::RunState;

/// The plan-step poller state (#96): the mtime guard over `plan.md` and the last
/// checkbox snapshot, diffed on each tick. The parse and the diff themselves live in
/// [`crate::plan_progress`], shared with the run-snapshot engine (#330).
#[derive(Default)]
pub(super) struct StepPoller {
    watch: PlanFileWatch,
    snapshot: Vec<PlanStep>,
}

impl StepPoller {
    /// Seed the snapshot from a just-folded `PlanWritten` (#96) so the initial plan
    /// state is the baseline — only later transitions emit. Called from the drain
    /// loop; leaves the mtime guard so the next `poll` still re-reads and reconciles.
    pub(super) fn reset_from_written(&mut self, steps: &[(String, String)]) {
        self.snapshot = steps
            .iter()
            .map(|(text, status)| PlanStep {
                text: plan_progress::normalize_step(text),
                status: StepStatus::from_wire(status),
            })
            .collect();
    }

    /// Poll `plan_path`: if its mtime advanced, re-parse the checkboxes, and for each
    /// step whose status moved TO `checked`/`noticed` (relative to the last snapshot)
    /// deliver a `dev.ralphy.plan.step` for the active issue. Best-effort — a stat or
    /// read failure is a silent no-op (the plan may not exist yet between issues).
    pub(super) fn poll<T: EventSink>(
        &mut self,
        transport: &T,
        ctx: &EventCtx,
        state: &RunState,
        plan_path: &Path,
        warned: &AtomicBool,
    ) {
        let Some(md) = self.watch.changed_text(plan_path) else {
            return;
        };
        let current = plan_progress::parse_steps(&md);
        if let Some(number) = state.active {
            for step in plan_progress::transitions(&self.snapshot, &current) {
                let ev = crate::events::envelope::plan_step_envelope(
                    ctx,
                    state,
                    number,
                    &step.text,
                    step.status.wire(),
                );
                deliver(transport, &ev, warned, RETRY_BASE_BACKOFF);
            }
        }
        self.snapshot = current;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::client::PostOutcome;
    use crate::runstate::RunEvent;
    use serde_json::Value;

    /// A test [`EventCtx`] with a stub emitter carrying a known `pid`.
    fn test_ctx() -> EventCtx {
        EventCtx {
            source: "ralphy/o/r".to_string(),
            runid: "01TESTRUNIDTESTRUNIDTE".to_string(),
            emitter: serde_json::json!({ "version": "0.0.0", "pid": 4242 }),
            git: serde_json::json!({ "repository": "o/r", "branch": "afk/run-t" }),
        }
    }

    /// A fake sink that records every delivered envelope for assertion.
    struct RecordingSink(std::sync::Mutex<Vec<Value>>);
    impl EventSink for RecordingSink {
        fn post(&self, body: &Value) -> anyhow::Result<PostOutcome> {
            self.0.lock().unwrap().push(body.clone());
            Ok(PostOutcome::Delivered)
        }
    }

    #[test]
    fn poller_emits_plan_step_on_checkbox_transition_and_reset_seeds_baseline() {
        // A temp plan.md (no `tempfile` dev-dep, per KNOWLEDGE): write it, seed the
        // poller, flip one step to `[x]`, bump the mtime, and assert exactly one
        // `plan.step` with the normalized text of the flipped step.
        let dir = std::env::temp_dir().join(format!("ralphy-step-poll-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan.md");
        std::fs::write(
            &plan_path,
            "## Steps\n- [ ] do a `thing`\n- [ ] do another\n",
        )
        .unwrap();

        // Issue 7 active so the poll has a subject.
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::IssueStarted {
            number: 7,
            title: "a".into(),
        });

        let sink = RecordingSink(std::sync::Mutex::new(Vec::new()));
        let warned = AtomicBool::new(false);
        let mut poller = StepPoller::default();

        // First poll seeds the snapshot (both steps open → nothing emitted).
        poller.poll(&sink, &test_ctx(), &state, &plan_path, &warned);
        assert!(
            sink.0.lock().unwrap().is_empty(),
            "no transitions on the seeding poll"
        );

        // Flip the first step to checked and advance the mtime so the poll re-reads.
        std::fs::write(
            &plan_path,
            "## Steps\n- [x] do a `thing`\n- [ ] do another\n",
        )
        .unwrap();
        crate::plan_progress::testutil::advance_mtime(&plan_path);
        poller.poll(&sink, &test_ctx(), &state, &plan_path, &warned);

        let delivered = sink.0.lock().unwrap();
        assert_eq!(delivered.len(), 1, "exactly one plan.step: {delivered:?}");
        let ev = &delivered[0];
        assert_eq!(ev["type"], "dev.ralphy.plan.step");
        assert_eq!(ev["subject"], "issue/7");
        assert_eq!(ev["data"]["status"], "checked");
        // The text is normalized (the backticks stripped).
        assert_eq!(ev["data"]["text"], "do a thing");
        // The subject-scoped issue block rides along.
        assert_eq!(ev["data"]["issue"]["number"], 7);
        drop(delivered);

        // `reset_from_written` re-baselines from a fold: a subsequent poll of the
        // same (already-checked) file emits nothing.
        poller.reset_from_written(&[
            ("do a `thing`".to_string(), "checked".to_string()),
            ("do another".to_string(), "open".to_string()),
        ]);
        crate::plan_progress::testutil::advance_mtime(&plan_path);
        poller.poll(&sink, &test_ctx(), &state, &plan_path, &warned);
        assert_eq!(
            sink.0.lock().unwrap().len(),
            1,
            "reset baseline suppresses the already-checked step"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `reset_from_written` on a FRESH poller: the baseline it seeds is what
    /// suppresses the emission, with no prior `poll` having set the snapshot.
    /// (In the test above the third poll would emit nothing even if this method
    /// were empty — the second poll already stored the checked state.)
    #[test]
    fn a_fold_seeded_baseline_suppresses_an_already_checked_step() {
        let dir = std::env::temp_dir().join(format!("ralphy-step-seed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan_path = dir.join("plan.md");
        std::fs::write(
            &plan_path,
            "## Steps\n- [x] do a `thing`\n- [!] surprised\n",
        )
        .unwrap();

        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::IssueStarted {
            number: 7,
            title: "a".into(),
        });
        let sink = RecordingSink(std::sync::Mutex::new(Vec::new()));
        let warned = AtomicBool::new(false);

        let mut poller = StepPoller::default();
        poller.reset_from_written(&[
            ("do a **thing**".to_string(), "checked".to_string()),
            ("surprised".to_string(), "noticed".to_string()),
        ]);
        poller.poll(&sink, &test_ctx(), &state, &plan_path, &warned);
        assert!(
            sink.0.lock().unwrap().is_empty(),
            "the fold's baseline suppresses both: {:?}",
            sink.0.lock().unwrap()
        );

        // The control: the SAME first poll without a seeded baseline emits both.
        let fresh = RecordingSink(std::sync::Mutex::new(Vec::new()));
        StepPoller::default().poll(&fresh, &test_ctx(), &state, &plan_path, &warned);
        assert_eq!(fresh.0.lock().unwrap().len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }
}
