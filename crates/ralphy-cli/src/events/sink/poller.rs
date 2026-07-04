//! The plan-step poller (#96): diffs a plan's checkbox lines against its last-seen
//! snapshot and delivers a `dev.ralphy.plan.step` on each checked/noticed transition.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use super::delivery::{deliver, RETRY_BASE_BACKOFF};
use crate::events::client::EventSink;
use crate::events::envelope::EventCtx;
use crate::runstate::RunState;

/// Normalize a checkbox step's text to its identity key (#96): drop markdown
/// emphasis/code markers and collapse runs of whitespace — mirroring
/// `ralphy_core::acceptance::normalize_ac`'s technique, kept crate-local so the poll
/// does not widen a core API for one caller.
fn normalize_step(s: &str) -> String {
    let stripped: String = s
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '`'))
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a plan's checkbox lines into `(normalized_text, status)` pairs (#96): a
/// `- [ ]` line is `open`, `- [x]`/`- [X]` is `checked`, `- [!]` is `noticed`. Used
/// by the plan-step poller to diff file states; the text is normalized so a
/// whitespace/emphasis edit that leaves a step's meaning unchanged is not a new step.
fn parse_checkbox_steps(md: &str) -> Vec<(String, &'static str)> {
    md.lines()
        .filter_map(|line| {
            let t = line.trim_start();
            let (status, rest) = if let Some(r) = t.strip_prefix("- [ ]") {
                ("open", r)
            } else if let Some(r) = t.strip_prefix("- [x]").or_else(|| t.strip_prefix("- [X]")) {
                ("checked", r)
            } else if let Some(r) = t.strip_prefix("- [!]") {
                ("noticed", r)
            } else {
                return None;
            };
            Some((normalize_step(rest), status))
        })
        .collect()
}

/// Map a `PlanWritten.steps` status string to the poller's `&'static str` status, so
/// the snapshot seeded from a fold is comparable to a parsed one.
fn static_status(s: &str) -> &'static str {
    match s {
        "checked" => "checked",
        "noticed" => "noticed",
        _ => "open",
    }
}

/// The plan-step poller state (#96): the last-seen `plan.md` mtime and the last
/// checkbox snapshot (`(normalized_text, status)`), diffed on each tick.
#[derive(Default)]
pub(super) struct StepPoller {
    last_mtime: Option<SystemTime>,
    snapshot: Vec<(String, &'static str)>,
}

impl StepPoller {
    /// Seed the snapshot from a just-folded `PlanWritten` (#96) so the initial plan
    /// state is the baseline — only later transitions emit. Called from the drain
    /// loop; leaves `last_mtime` so the next `poll` still re-reads and reconciles.
    pub(super) fn reset_from_written(&mut self, steps: &[(String, String)]) {
        self.snapshot = steps
            .iter()
            .map(|(text, status)| (normalize_step(text), static_status(status)))
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
        let Ok(mtime) = std::fs::metadata(plan_path).and_then(|m| m.modified()) else {
            return;
        };
        if self.last_mtime == Some(mtime) {
            return; // unchanged since the last poll
        }
        self.last_mtime = Some(mtime);
        let Ok(md) = std::fs::read_to_string(plan_path) else {
            return;
        };
        let current = parse_checkbox_steps(&md);
        if let Some(number) = state.active {
            for (text, status) in &current {
                if *status != "checked" && *status != "noticed" {
                    continue;
                }
                let prev = self
                    .snapshot
                    .iter()
                    .find(|(t, _)| t == text)
                    .map(|(_, s)| *s);
                if prev == Some(*status) {
                    continue; // no transition
                }
                let ev = crate::events::envelope::plan_step_envelope(ctx, state, number, text, status);
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
    use std::time::{Duration, Instant};

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
        filetime_advance(&plan_path);
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
        filetime_advance(&plan_path);
        poller.poll(&sink, &test_ctx(), &state, &plan_path, &warned);
        assert_eq!(
            sink.0.lock().unwrap().len(),
            1,
            "reset baseline suppresses the already-checked step"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Bump a file's mtime forward so the poller's `mtime` guard sees a change even
    /// when the two writes land in the same clock tick (coarse FS timestamps).
    fn filetime_advance(path: &Path) {
        let now = SystemTime::now() + Duration::from_secs(2);
        // Re-write with an explicit later mtime via a set_modified where available;
        // fall back to a spin until the OS mtime actually advances.
        if std::fs::File::open(path)
            .and_then(|f| f.set_modified(now))
            .is_err()
        {
            let start = Instant::now();
            let initial = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            while std::fs::metadata(path).and_then(|m| m.modified()).ok() == initial {
                if start.elapsed() > Duration::from_secs(3) {
                    break;
                }
                std::fs::write(path, std::fs::read(path).unwrap()).ok();
            }
        }
    }
}
