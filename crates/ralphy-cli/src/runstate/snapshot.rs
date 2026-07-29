//! The **pure** projection from the runstate fold to the run-snapshot document
//! (ADR-0047 §5): `(SnapshotCtx, RunState) -> RunSnapshot`.
//!
//! No process, no clock, no filesystem — everything time- or environment-derived
//! arrives in [`SnapshotCtx`], minted once at run boot. That is what lets this be
//! unit-tested like the CloudEvents envelope mapper and the fold itself; the
//! writing lives in `run::snapshot_engine`.

use ralphy_run_snapshot::{
    IssueBlock, PhaseBlock, PlanBlock, PlanStepBlock, QueueBlock, RunSnapshot, SleepBlock,
};

use super::{IssueStatus, RunState};
use crate::plan_progress::PlanProgress;

/// The run facts the fold does not carry: identity, where it runs, and when it
/// started. Constant for the life of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotCtx {
    pub runid: String,
    pub pid: u32,
    /// The repo slug (`owner/name`), matching the events `git.repository`.
    pub repo: String,
    pub branch: String,
    /// RFC 3339, local offset — the same shape the run lock records.
    pub started_at: String,
    /// REPO-RELATIVE, never absolute and never the plan's text: what the browser
    /// hands straight to the confined `file.read` verb (ADR-0047 §5).
    pub plan_path: String,
}

/// Project `state` into the document a reader renders.
///
/// The `issues` array is the WHOLE queue trail in [`RunState::order`] order —
/// folded entries carry their status, queue entries never entered carry
/// `"pending"` — so the panel's trail shows queued issues without the browser
/// merging anything.
///
/// `plan` arrives as an argument rather than out of [`RunState`]: it is polled
/// from the filesystem, and the fold is built from `tracing` events only (#330).
pub fn project(ctx: &SnapshotCtx, state: &RunState, plan: &PlanProgress) -> RunSnapshot {
    RunSnapshot {
        v: ralphy_run_snapshot::SNAPSHOT_VERSION,
        runid: ctx.runid.clone(),
        pid: ctx.pid,
        title: state.title.clone(),
        repo: ctx.repo.clone(),
        branch: ctx.branch.clone(),
        plan_agent: state.plan_agent.clone(),
        exec_agent: state.exec_agent.clone(),
        started_at: ctx.started_at.clone(),
        plan_path: ctx.plan_path.clone(),
        queue: QueueBlock {
            total: state.total,
            order: state.order.clone(),
            stop_before: state.stop_before,
        },
        issues: trail(state),
        phase: PhaseBlock {
            active: state.active,
            state: state.run_phase().to_string(),
            // Always `None` here, and that is the module's purity holding: the
            // phase clock's anchor is a wall-clock instant, and this function has
            // no clock. `run::snapshot_engine` stamps it, because only something
            // comparing consecutive projections can tell a phase CHANGE from a
            // phase, and only a changed phase may restamp the anchor.
            since: None,
            sleep: state.sleep.as_ref().map(|s| SleepBlock {
                reset: (!s.reset.is_empty()).then(|| s.reset.clone()),
                target_epoch: s.target_epoch,
            }),
            final_summary: state.final_summary.clone(),
        },
        plan: plan_block(plan, state),
    }
}

/// The plan block, keyed to the active issue: a plan armed for a DIFFERENT issue
/// (the previous one's, still on disk between issues) projects as absent. The
/// engine arms off the plan EVENT's own issue number, so this check is an
/// independent second opinion — when the two disagree the document carries no
/// plan at all, never the wrong issue's (#330).
fn plan_block(plan: &PlanProgress, state: &RunState) -> PlanBlock {
    if plan.issue.is_none() || plan.issue != state.active {
        return PlanBlock::default();
    }
    PlanBlock {
        issue: plan.issue,
        steps: plan
            .steps
            .iter()
            .map(|s| PlanStepBlock {
                text: s.text.clone(),
                status: s.status.wire().to_string(),
            })
            .collect(),
    }
}

/// The merged issue trail: the working order first (falling back to the light
/// queue scope when `queue.built` carried no order), then any folded issue
/// neither names — a forced `--only-issue` run has issues outside the order.
fn trail(state: &RunState) -> Vec<IssueBlock> {
    let mut numbers: Vec<u64> = if state.order.is_empty() {
        state.queue.iter().map(|q| q.number).collect()
    } else {
        state.order.clone()
    };
    for entry in &state.issues {
        if !numbers.contains(&entry.number) {
            numbers.push(entry.number);
        }
    }
    numbers.into_iter().map(|n| block(state, n)).collect()
}

fn block(state: &RunState, number: u64) -> IssueBlock {
    let queue_title = state
        .queue
        .iter()
        .find(|q| q.number == number)
        .map(|q| q.title.clone());
    match state.issues.iter().find(|e| e.number == number) {
        Some(e) => IssueBlock {
            number,
            // The fold seeds an entry with an empty title when the issue was
            // materialized by a numberless adapter event; the queue scope knows it.
            title: if e.title.is_empty() {
                queue_title.unwrap_or_default()
            } else {
                e.title.clone()
            },
            status: status_wire(&e.status).to_string(),
            kind: e.kind.map(|k| k.skip_wire().to_string()),
            blocked_by: e.blocked_by.clone(),
            model: e.model.clone(),
            effort: e.effort.clone(),
            budget_min: e.budget_min,
        },
        None => IssueBlock {
            number,
            title: queue_title.unwrap_or_default(),
            status: "pending".to_string(),
            ..IssueBlock::default()
        },
    }
}

/// The terminal statuses use `status_wire` so the snapshot and the
/// `run.finished.issues` rollup cannot drift (ADR-0047 §5); the two non-terminal
/// ones get the phase vocabulary the panel already renders.
fn status_wire(status: &IssueStatus) -> &'static str {
    status.status_wire().unwrap_or(match status {
        IssueStatus::Executing => "executing",
        _ => "planning",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runstate::{RunEvent, SkipKind, UsageLite};

    fn ctx() -> SnapshotCtx {
        SnapshotCtx {
            runid: "01TESTRUNID".into(),
            pid: 4242,
            repo: "owner/repo".into(),
            branch: "afk/run-1".into(),
            started_at: "2026-07-24T10:00:00-03:00".into(),
            plan_path: ".ralphy/plan.md".into(),
        }
    }

    /// A 3-issue queue where only #1 and #2 ever entered the lifecycle.
    fn two_of_three() -> RunState {
        let mut state = RunState::new("three issues", 3);
        state.apply(RunEvent::QueueBuilt {
            count: 3,
            order: vec![1, 2, 3],
            stop_before: Some(3),
            issues: serde_json::json!([
                {"number": 1, "title": "one"},
                {"number": 2, "title": "two"},
                {"number": 3, "title": "three"},
            ]),
            assignee_filter: None,
            scope: None,
        });
        state.apply(RunEvent::IssueStarted {
            number: 1,
            title: "one".into(),
        });
        state.apply(RunEvent::IssueClosed {
            number: 1,
            tokens: 0,
            invocations: 0,
            usage: UsageLite::default(),
        });
        state.apply(RunEvent::IssueStarted {
            number: 2,
            title: "two".into(),
        });
        state.apply(RunEvent::Executing {
            number: 2,
            budget_min: 45,
            model: "opus".into(),
            effort: Some("high".into()),
        });
        state
    }

    #[test]
    fn project_merges_queue_order_with_folded_statuses() {
        let snap = project(&ctx(), &two_of_three(), &PlanProgress::default());
        assert_eq!(
            snap.issues
                .iter()
                .map(|i| i.status.as_str())
                .collect::<Vec<_>>(),
            ["done", "executing", "pending"],
            "the whole queue trail, not only the entered issues"
        );
        assert_eq!(
            snap.issues.iter().map(|i| i.number).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(
            snap.issues[2].title, "three",
            "a never-entered issue takes its title from the queue scope"
        );
        assert_eq!(snap.issues[1].model.as_deref(), Some("opus"));
        assert_eq!(snap.issues[1].budget_min, Some(45));
        assert_eq!(snap.queue.total, 3);
        assert_eq!(snap.queue.order, vec![1, 2, 3]);
        assert_eq!(snap.queue.stop_before, Some(3));
        assert_eq!(snap.phase.active, Some(2));
        assert_eq!(snap.phase.state, "executing");
        assert_eq!(
            snap.phase.since, None,
            "the projection has no clock — the writer stamps the phase anchor"
        );
        assert_eq!(snap.v, ralphy_run_snapshot::SNAPSHOT_VERSION);
        assert_eq!(snap.plan_path, ".ralphy/plan.md");
    }

    #[test]
    fn project_carries_sleep_reset_and_target_epoch() {
        let mut state = two_of_three();
        state.apply(RunEvent::SleepStarted {
            reset: "14:30".into(),
            target_epoch: 1_700_000_000,
        });
        let snap = project(&ctx(), &state, &PlanProgress::default());
        assert_eq!(snap.phase.state, "sleeping");
        let sleep = snap.phase.sleep.expect("a sleep block");
        assert_eq!(sleep.reset.as_deref(), Some("14:30"));
        assert_eq!(sleep.target_epoch, 1_700_000_000);
    }

    #[test]
    fn project_carries_skip_kind_and_blockers() {
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::QueueBuilt {
            count: 1,
            order: vec![9],
            stop_before: None,
            issues: serde_json::Value::Null,
            assignee_filter: None,
            scope: None,
        });
        state.apply(RunEvent::Skipped {
            number: 9,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![7],
        });
        let snap = project(&ctx(), &state, &PlanProgress::default());
        assert_eq!(snap.issues[0].status, "skipped");
        assert_eq!(snap.issues[0].kind.as_deref(), Some("blocked_by"));
        assert_eq!(snap.issues[0].blocked_by, vec![7]);
    }

    #[test]
    fn project_appends_a_folded_issue_outside_the_order() {
        // `--only-issue` runs an issue the built order never named.
        let mut state = RunState::new("t", 1);
        state.apply(RunEvent::IssueStarted {
            number: 71,
            title: "forced".into(),
        });
        let snap = project(&ctx(), &state, &PlanProgress::default());
        assert_eq!(snap.issues.len(), 1);
        assert_eq!(snap.issues[0].number, 71);
        assert_eq!(snap.issues[0].status, "planning");
    }

    #[test]
    fn plan_block_carries_steps_and_issue() {
        // #2 is the active issue in `two_of_three()`.
        let plan = PlanProgress {
            issue: Some(2),
            steps: crate::plan_progress::parse_steps("- [ ] first step\n- [x] second step\n"),
        };
        let snap = project(&ctx(), &two_of_three(), &plan);
        assert_eq!(snap.plan.issue, Some(2));
        assert_eq!(snap.plan.steps.len(), 2);
        assert_eq!(snap.plan.steps[0].text, "first step");
        assert_eq!(snap.plan.steps[0].status, "open");
        assert_eq!(snap.plan.steps[1].status, "checked");
    }

    #[test]
    fn a_previous_issues_plan_is_not_projected() {
        // Between issues `plan.md` still holds #1's plan; #2 is active. Without the
        // keying guard #1's steps would surface as #2's progress.
        let plan = PlanProgress {
            issue: Some(1),
            steps: crate::plan_progress::parse_steps("- [x] previous issue's step\n"),
        };
        let snap = project(&ctx(), &two_of_three(), &plan);
        assert_eq!(snap.plan, ralphy_run_snapshot::PlanBlock::default());
        assert_eq!(snap.plan.issue, None);
        assert!(snap.plan.steps.is_empty());
    }

    /// The closed-vocabulary gate for step statuses, the sibling of
    /// [`every_issue_status_is_known_to_the_runs_panel`]: the document ships
    /// `plan.steps[].status` as a string with no compiler between it and the panel.
    #[test]
    fn every_step_status_is_known_to_the_runs_panel() {
        use crate::plan_progress::StepStatus;
        const PANEL: &str = include_str!("../../../ralphy-daemon/assets/ui/wb-runs.js");
        let all = [StepStatus::Open, StepStatus::Checked, StepStatus::Noticed];
        for status in all {
            // Exhaustiveness guard: a new variant stops this match compiling.
            match status {
                StepStatus::Open | StepStatus::Checked | StepStatus::Noticed => {}
            }
            let wire = status.wire();
            // BOTH tables, not "somewhere in the file": each wire word appears
            // once per table, so a whole-file `contains` stays green when one of
            // them loses the key and the panel silently falls back to `open`.
            for table in ["STEP_GLYPH: {", "STEP_LABEL: {"] {
                let from = PANEL
                    .split_once(table)
                    .expect("wb-runs.js declares {table}")
                    .1;
                let body = from.split_once("\n  },").expect("the table is closed").0;
                assert!(
                    body.contains(&format!("{wire}: \"")),
                    "wb-runs.js `{table}` has no `{wire}` key — the panel would fall back to open"
                );
            }
        }
    }

    /// Every `IssueStatus`, with an exhaustiveness guard: a new variant stops
    /// this match compiling, so the pin below can never silently miss one.
    fn all_statuses() -> Vec<IssueStatus> {
        let all = vec![
            IssueStatus::Planning,
            IssueStatus::Executing,
            IssueStatus::Planned,
            IssueStatus::Done,
            IssueStatus::Skipped,
            IssueStatus::Blocked,
            IssueStatus::Infeasible,
            IssueStatus::NeedsSplit,
            IssueStatus::NonGreen,
            IssueStatus::Hitl,
        ];
        for s in &all {
            match s {
                IssueStatus::Planning
                | IssueStatus::Executing
                | IssueStatus::Planned
                | IssueStatus::Done
                | IssueStatus::Skipped
                | IssueStatus::Blocked
                | IssueStatus::Infeasible
                | IssueStatus::NeedsSplit
                | IssueStatus::NonGreen
                | IssueStatus::Hitl => {}
            }
        }
        all
    }

    /// The closed-vocabulary gate (ADR-0047 §10, PRD #296's defect class): the
    /// snapshot ships `status` as a string, and the Runs panel is the consumer
    /// with no compiler between them. `planned` was missing from all three JS
    /// tables and rendered as `○ pending`, undercounting the progress counter.
    /// Every status the projection can emit must be known to the panel.
    #[test]
    fn every_issue_status_is_known_to_the_runs_panel() {
        const PANEL: &str = include_str!("../../../ralphy-daemon/assets/ui/wb-runs.js");
        let terminal_line = PANEL
            .lines()
            .find(|l| l.contains("TERMINAL: new Set("))
            .expect("wb-runs.js declares a TERMINAL set");
        for status in all_statuses() {
            let wire = status_wire(&status);
            assert!(
                PANEL.contains(&format!("{wire}: \"")),
                "wb-runs.js GLYPH/LABEL has no `{wire}` key — the panel would render it as pending"
            );
            assert_eq!(
                terminal_line.contains(&format!("\"{wire}\"")),
                status.is_terminal(),
                "wb-runs.js TERMINAL disagrees with IssueStatus::is_terminal on `{wire}`"
            );
        }
    }

    #[test]
    fn project_is_pure_over_runstate() {
        // Same state in, byte-identical document out: no clock, no pid read, no
        // filesystem hides in the projection.
        let state = two_of_three();
        let ctx = ctx();
        let a = serde_json::to_string(&project(&ctx, &state, &PlanProgress::default())).unwrap();
        let b = serde_json::to_string(&project(&ctx, &state, &PlanProgress::default())).unwrap();
        assert_eq!(a, b);
    }
}
