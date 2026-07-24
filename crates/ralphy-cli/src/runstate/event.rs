//! The semantic-event mapper: [`RunEvent`] plus [`event_to_runevent`], which lifts
//! the raw `(target, message, fields)` triple into a typed lifecycle event
//! (ADR-0007 D6).
//!
//! **The convention** (ADR-0039 §1/§2): every arm matches a `ralphy_core::emit`
//! constant, never a message literal — the emitter and the decoder read the SAME
//! `…_MSG`, so a renamed message cannot half-land. A new [`RunEvent`] variant
//! without an `emit` helper AND a round-trip test in [`super::roundtrip`] is an
//! incomplete change; `roundtrip::_every_variant_has_a_roundtrip` enforces it at
//! compile time.
//!
//! No message literal remains: the vendor adapters' phase events collapsed into
//! `emit::planning` / `emit::executing` plus a `cmd` field (ADR-0039 Decision 3),
//! so every arm below reads a constant.

use tracing::Level;

use super::fields::{usage_from, EventFields};
use super::{SkipKind, UsageLite};

/// One semantic run event, already lifted out of the raw `(target, message,
/// fields)` triple by [`event_to_runevent`]. One variant per consumed lifecycle
/// event.
///
/// Not `Eq`: [`RunEvent::RunStarted`] carries `deadline_hours: Option<f64>`, and
/// `f64` has no total equality. `PartialEq` is all any consumer needs (`assert_eq!`
/// in the fold/decoder tests, no `HashSet`/`BTreeSet` of events).
#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    /// The queue was built: its size, the issue numbers in order, the first
    /// issue carrying `stop-before` (where the run will halt), if any, and the
    /// enriched per-issue snapshot (ADR-0020) — a `serde_json::Value` array of
    /// `{number, title, labels[], queue_status, skip_reason?, blocked_by[],
    /// position?}`, or `Value::Null` when the resolver produced none (the legacy
    /// shape). The Telegram/console fold ignores `issues`; only the CloudEvents
    /// sink carries it onto `queue.built`.
    QueueBuilt {
        count: u64,
        order: Vec<u64>,
        stop_before: Option<u64>,
        issues: serde_json::Value,
        /// The resolved concrete login the queue was scoped to (ADR-0021 §5);
        /// `None` = whole queue (unfiltered, or an explicit `--issues`/`--only-issue`
        /// selection). Only the CloudEvents sink carries it onto `queue.built`.
        assignee_filter: Option<String>,
        /// The human-readable queue scope phrase (`labels [AFK]`, `issue #7`…),
        /// LOG-ONLY (#222): the console edge notice is folded from it and it is
        /// deliberately NOT mapped onto the `queue.built` envelope.
        scope: Option<String>,
    },
    /// Work began on an issue (number + title).
    IssueStarted { number: u64, title: String },
    /// The planning phase started for the active issue (adapter event). Carries
    /// the planner's display model/effort so the live region can label the
    /// planning spinner and the `plan written` scroll line. Live-region only —
    /// no permanent line; the adapter never learns the issue number.
    Planning {
        model: Option<String>,
        effort: Option<String>,
    },
    /// A plan was written; `open_steps == 0` means the plan is infeasible. The
    /// `usage` is the planning phase's token consumption for the inline meter.
    /// `steps` is the full `(text, status)` checkbox list parsed off the runner's
    /// `steps_json` field, carried onto `plan.written.data.steps` (#96).
    PlanWritten {
        number: u64,
        open_steps: u64,
        usage: UsageLite,
        steps: Vec<(String, String)>,
    },
    /// Execution started for the active issue. The adapter never learns the issue
    /// number, so `number` is `0` here and resolves to the active issue.
    Executing {
        number: u64,
        budget_min: u64,
        model: String,
        effort: Option<String>,
    },
    /// A green issue was closed (the cycle). `tokens` is the issue's total (plan +
    /// execute) flat count, kept for the telegram notifier; `usage` is the
    /// *execution* phase's breakdown, which the live region combines with the
    /// planning usage it stashed at `PlanWritten` to show the issue total (D11).
    IssueClosed {
        number: u64,
        tokens: u64,
        /// Vendor spawns this issue paid for (plan + execute + any repair/protocol
        /// bounce). Defaults to `0` on the decoder-absent path (an older producer or a
        /// manual construction) — additive and round-trip tolerant.
        invocations: u64,
        usage: UsageLite,
    },
    /// An issue finished non-green and stopped the run; `outcome` is the core's
    /// `Outcome` debug string (e.g. `Stuck`, `Blocked`, `Timeout`).
    NonGreen { number: u64, outcome: String },
    /// An issue was skipped (blocked-by an open issue, a `stop-before` label, a
    /// human-return label, or a verify gate still red after the repair budget).
    /// `label` names the parking label on a [`SkipKind::HumanReturn`] skip (so the
    /// operator sees exactly which label parked it); `None` for the other kinds.
    /// `blockers` names the still-open issue(s) that gated a [`SkipKind::BlockedBy`]
    /// skip (so the line reads `skipped (blocked by #139)`); empty for the other kinds.
    Skipped {
        number: u64,
        kind: SkipKind,
        label: Option<String>,
        blockers: Vec<u64>,
    },
    /// An issue is stalled on a human gate (`ready-for-human`/`HITL`) in its
    /// dependency path (ADR-0014): `on` names the human-blocker issue(s) a person
    /// must act on. The run continues — only this chain waits — but the operator
    /// needs to see *which* issue is theirs to clear.
    HumanBlocked { number: u64, on: Vec<u64> },
    /// The planner judged the issue a bundle (several backlog tasks under one
    /// number): the queue is parked on a human split. Follows the infeasible
    /// `PlanWritten { open_steps: 0 }` and upgrades the status.
    NeedsSplit { number: u64 },
    /// A WARN or ERROR event from the run: level wins over message content.
    Notice { level: Level, message: String },
    /// The deadline passed before this issue could be started.
    DeadlinePassed { number: u64 },
    /// The run hit a usage limit and is sleeping until `reset`; `target_epoch` is
    /// the Unix-seconds wake anchor (the reset plus the wait-policy buffer) for a
    /// live countdown.
    SleepStarted { reset: String, target_epoch: i64 },
    /// The reset arrived and the run resumed; clears any active sleep.
    SleepEnded,
    /// The active issue's child hit a sustained API failure (banner persisted
    /// ≥ 3 min): the adapter is retrying. Live-region only (retry indicator) plus
    /// a matched Telegram/CloudEvents ping. All the "is this a failure" timing
    /// lives in the adapter; the sink just reacts.
    ApiDegraded,
    /// The child's API recovered (transcript activity resumed) after an
    /// [`RunEvent::ApiDegraded`] — always a matched pair, never emitted alone.
    ApiRecovered,
    /// The idle watchdog reaped the active issue's child after `idle_minutes`
    /// with no progress (docs/adr/0038). Emitted identically from the PTY and the
    /// headless paths — the two measure progress differently, but the operator
    /// sees one event either way. Distinguishes "died of silence" from the wall
    /// clock, which is otherwise invisible: both surface as `Timeout`.
    IdleReaped { idle_minutes: u64 },
    /// The end-of-run knowledge consolidation started, folding `notes` loose
    /// per-issue notes into `KNOWLEDGE.md`.
    KnowledgeConsolidating { notes: u64 },
    /// Knowledge consolidation finished, archiving `archived` notes into
    /// `knowledge/raw/` after curating `KNOWLEDGE.md`.
    KnowledgeConsolidated { archived: u64 },
    /// The run began working a queue (ADR-0019 boundary event, emitted by the CLI
    /// after branch-mode/base-branch resolution). Carries the CLI-only run
    /// parameters (labels, agents, branch policy, deadline) the core never sees.
    RunStarted {
        repo: String,
        queue_labels: Vec<String>,
        agent: String,
        plan_agent: String,
        branch_mode: String,
        branch: String,
        deadline_hours: Option<f64>,
    },
    /// The run ended cleanly (ADR-0019 boundary event, emitted by the CLI only when
    /// `run_queue` returns `Ok`). `outcome` is the mapped queue-stop label; the
    /// totals summarize the whole run.
    RunFinished {
        outcome: String,
        issues_done: u64,
        issues_skipped: u64,
        issues_total: u64,
        issues_blocked: u64,
        issues_hitl: u64,
        /// The run's OWN per-issue rollup (a JSON array of `{number, status,
        /// kind?, blocked_by?}`), the same fold the scalars come from.
        /// `Value::Null` when the emitter carried none — the envelope then falls
        /// back to the folded [`super::RunState`].
        issues: serde_json::Value,
        up: u64,
        cr: u64,
        cw: u64,
        out: u64,
        duration_s: u64,
    },
    /// The run declined to start and returned cleanly (#222) — `--if-idle`
    /// deferring to a live run. Mapped to `dev.ralphy.run.skipped`.
    RunSkipped { reason: String },
    /// The raw `plan.md` snapshot at the plan-write point (#96), mapped to
    /// `dev.ralphy.plan.opened`. Issue-scoped; carries only the number + raw
    /// markdown, so it keeps `RunEvent` `PartialEq` (no new `Eq`/hash requirement).
    PlanOpened { number: u64, plan_md: String },
    /// The raw `plan.md` snapshot captured at the issue close, before the next
    /// issue's `plan()` overwrites it (#96), mapped to `dev.ralphy.plan.closed`.
    PlanClosed { number: u64, plan_md: String },
}

/// Map an event's `(target, message, fields)` to a [`RunEvent`], or `None` for an
/// event the run ignores. Pure over its inputs and unit-tested per consumed event
/// so an event/model drift fails a test (ADR-0007 D6).
///
/// Level wins: a WARN or ERROR event emits [`RunEvent::Notice`] regardless of its
/// message content, so a warning can never silently vanish into an unmatched arm.
///
/// `target` is currently informational — the message + fields uniquely identify
/// every consumed event — but kept in the signature for future disambiguation.
pub fn event_to_runevent(target: &str, message: &str, fields: &EventFields) -> Option<RunEvent> {
    let _ = target;
    // Level wins: WARN and ERROR always surface as Notice.
    if fields.level == Level::WARN || fields.level == Level::ERROR {
        return Some(RunEvent::Notice {
            level: fields.level,
            message: message.to_string(),
        });
    }
    let number = fields.number.unwrap_or(0);
    match message {
        ralphy_core::emit::QUEUE_BUILT_MSG => Some(RunEvent::QueueBuilt {
            count: fields.count.unwrap_or(0),
            order: parse_order(fields.order.as_deref()),
            // 0 is the "no stop-before in this queue" sentinel (issue numbers are ≥1).
            stop_before: fields.stop_before.filter(|&n| n != 0),
            // The enriched per-issue snapshot (ADR-0020): parse the JSON array
            // string, falling back to `Null` when absent or unparseable so a
            // legacy emitter (or a snapshot-build failure) still decodes cleanly.
            issues: parse_issues_snapshot(fields.issues_json.as_deref()),
            // The resolved concrete login the queue was scoped to (ADR-0021 §5);
            // `None` when the queue was fetched unfiltered.
            assignee_filter: fields.assignee_filter.clone(),
            // LOG-ONLY (#222): the console edge notice's scope phrase; `""` folds
            // to `None` and no envelope arm reads it.
            scope: fields.scope.clone(),
        }),
        ralphy_core::emit::ISSUE_STARTED_MSG => Some(RunEvent::IssueStarted {
            number,
            title: fields.title.clone().unwrap_or_default(),
        }),
        // The adapter's planning events carry no issue number; the fold applies
        // the display model/effort to the active issue's planning spinner.
        ralphy_core::emit::PLANNING_MSG => Some(RunEvent::Planning {
            model: fields.model.clone(),
            effort: fields.effort.clone(),
        }),
        ralphy_core::emit::PLAN_WRITTEN_MSG => Some(RunEvent::PlanWritten {
            number,
            open_steps: fields.open_steps.unwrap_or(0),
            usage: usage_from(fields),
            steps: parse_steps_json(fields.steps_json.as_deref()),
        }),
        // The raw plan snapshots (#96): issue-scoped, carrying the complete `plan.md`.
        ralphy_core::emit::PLAN_OPENED_MSG => Some(RunEvent::PlanOpened {
            number,
            plan_md: fields.plan_md.clone().unwrap_or_default(),
        }),
        ralphy_core::emit::PLAN_CLOSED_MSG => Some(RunEvent::PlanClosed {
            number,
            plan_md: fields.plan_md.clone().unwrap_or_default(),
        }),
        // The adapter's execution events carry no issue number; the fold applies
        // this to the active issue.
        ralphy_core::emit::EXECUTING_MSG => Some(RunEvent::Executing {
            number,
            budget_min: fields.budget_min.unwrap_or(0),
            model: fields.model.clone().unwrap_or_default(),
            effort: fields.effort.clone(),
        }),
        ralphy_core::emit::ISSUE_CLOSED_MSG => Some(RunEvent::IssueClosed {
            number,
            tokens: fields.tokens.unwrap_or(0),
            invocations: fields.invocations.unwrap_or(0),
            usage: usage_from(fields),
        }),
        ralphy_core::emit::NON_GREEN_MSG => Some(RunEvent::NonGreen {
            number,
            outcome: fields.outcome.clone().unwrap_or_default(),
        }),
        ralphy_core::emit::NEEDS_SPLIT_MSG => Some(RunEvent::NeedsSplit { number }),
        ralphy_core::emit::BLOCKED_BY_OPEN_MSG => Some(RunEvent::Skipped {
            number,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: parse_u64_list(fields.blockers.as_deref()),
        }),
        // A human gate (`ready-for-human`/`HITL`) sits in the issue's path: the
        // chain is parked until a person acts, but the run continues. `on` names
        // the issue(s) the operator must clear (ADR-0014).
        ralphy_core::emit::BLOCKED_WAITING_HUMAN_MSG => Some(RunEvent::HumanBlocked {
            number,
            on: parse_u64_list(fields.human_blockers.as_deref()),
        }),
        ralphy_core::emit::STOP_BEFORE_LABEL_MSG => Some(RunEvent::Skipped {
            number,
            kind: SkipKind::StopBefore,
            label: None,
            blockers: Vec::new(),
        }),
        // A human-return label (`ready-for-human`/`HITL`, `needs-info`,
        // `needs-triage`, `wontfix`, `triage-agent`) outranks the queue label: the
        // issue is skipped with the parking label named and the queue continues
        // (ADR-0016).
        ralphy_core::emit::HUMAN_RETURN_LABEL_MSG => Some(RunEvent::Skipped {
            number,
            kind: SkipKind::HumanReturn,
            label: fields.label.clone(),
            blockers: Vec::new(),
        }),
        // The verify gate stayed red after the repair budget: the issue is left
        // open and the queue marches on (ADR-0011). Surfaced as a skip so the miss
        // is visible in the live card and the final counts.
        ralphy_core::emit::VERIFY_GATE_FAILED_MSG => Some(RunEvent::Skipped {
            number,
            kind: SkipKind::VerifyFailed,
            label: None,
            blockers: Vec::new(),
        }),
        ralphy_core::emit::DEADLINE_PASSED_MSG => Some(RunEvent::DeadlinePassed { number }),
        // The run entered a usage-limit sleep; the fold carries the reset hint and
        // the wake anchor for a live countdown.
        ralphy_core::emit::USAGE_LIMIT_WAITING_MSG => Some(RunEvent::SleepStarted {
            reset: fields.reset.clone().unwrap_or_default(),
            target_epoch: fields.target_epoch.unwrap_or(0),
        }),
        ralphy_core::emit::RESET_REACHED_MSG => Some(RunEvent::SleepEnded),
        // The API-degraded transitions, from EITHER execution path (PTY #149,
        // headless #217): all timing gating happens in the adapter, so these fire
        // only on the real edges. The messages are shared constants so the two
        // emitters cannot drift into two different operator experiences.
        ralphy_core::emit::API_DEGRADED_MSG => Some(RunEvent::ApiDegraded),
        ralphy_core::emit::API_RECOVERED_MSG => Some(RunEvent::ApiRecovered),
        // The idle watchdog's reap, from either execution path — the message is
        // one shared constant (`ralphy_core::emit::IDLE_REAPED_MSG`) so the
        // two emitters cannot drift apart into two different operator experiences.
        ralphy_core::emit::IDLE_REAPED_MSG => Some(RunEvent::IdleReaped {
            idle_minutes: fields.idle_minutes.unwrap_or(0),
        }),
        // The end-of-run knowledge consolidation trigger: both events reuse the
        // generic `count` field (notes in / notes archived).
        ralphy_core::emit::KNOWLEDGE_CONSOLIDATING_MSG => Some(RunEvent::KnowledgeConsolidating {
            notes: fields.count.unwrap_or(0),
        }),
        ralphy_core::emit::KNOWLEDGE_CONSOLIDATED_MSG => Some(RunEvent::KnowledgeConsolidated {
            archived: fields.count.unwrap_or(0),
        }),
        // The two ADR-0019 run-boundary emissions (from the CLI, not the core).
        ralphy_core::emit::RUN_STARTED_MSG => Some(RunEvent::RunStarted {
            repo: fields.repo.clone().unwrap_or_default(),
            queue_labels: split_labels(fields.queue_labels.as_deref()),
            agent: fields.agent.clone().unwrap_or_default(),
            plan_agent: fields.plan_agent.clone().unwrap_or_default(),
            branch_mode: fields.branch_mode.clone().unwrap_or_default(),
            branch: fields.base.clone().unwrap_or_default(),
            // `0.0` is the "no deadline" sentinel the emitter uses (an absent
            // `--deadline-hours` becomes `0.0`), so filter it back to `None`.
            deadline_hours: fields.deadline_hours.filter(|&h| h > 0.0),
        }),
        ralphy_core::emit::RUN_FINISHED_MSG => Some(RunEvent::RunFinished {
            outcome: fields.outcome.clone().unwrap_or_default(),
            issues_done: fields.issues_done.unwrap_or(0),
            issues_skipped: fields.issues_skipped.unwrap_or(0),
            issues_total: fields.issues_total.unwrap_or(0),
            issues_blocked: fields.issues_blocked.unwrap_or(0),
            issues_hitl: fields.issues_hitl.unwrap_or(0),
            issues: parse_issues_snapshot(fields.issues_json.as_deref()),
            up: fields.up.unwrap_or(0),
            cr: fields.cr.unwrap_or(0),
            cw: fields.cw.unwrap_or(0),
            out: fields.out.unwrap_or(0),
            duration_s: fields.duration_s.unwrap_or(0),
        }),
        ralphy_core::emit::RUN_SKIPPED_MSG => Some(RunEvent::RunSkipped {
            reason: fields.reason.clone().unwrap_or_default(),
        }),
        _ => None,
    }
}

/// Split the comma-joined `queue_labels` field into the typed list, dropping empty
/// tokens (an empty joined string yields an empty list, not a `[""]`).
fn split_labels(raw: Option<&str>) -> Vec<String> {
    match raw {
        None => Vec::new(),
        Some(s) => s
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect(),
    }
}

/// Parse the enriched `queue built` snapshot (ADR-0020): the `issues_json` field
/// is the JSON array string the CLI serialized from `resolve_queue_view`. Returns
/// `Value::Null` when absent or unparseable, so a legacy `queue built` (no
/// snapshot) or a snapshot-build failure never breaks decoding — the sink then
/// emits the legacy `queue.built` shape.
fn parse_issues_snapshot(raw: Option<&str>) -> serde_json::Value {
    raw.and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Parse the runner's `steps_json` field (`[{text,status}]`) into the typed
/// `(text, status)` list on `PlanWritten` (#96). Empty (an absent field or a parse
/// failure) so a legacy `plan written` with no steps still decodes cleanly.
fn parse_steps_json(raw: Option<&str>) -> Vec<(String, String)> {
    let Some(s) = raw else {
        return Vec::new();
    };
    let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<serde_json::Value>(s) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|e| {
            let text = e.get("text")?.as_str()?.to_string();
            let status = e.get("status")?.as_str()?.to_string();
            Some((text, status))
        })
        .collect()
}

/// Parse the `queue built` `order` field (`#30 -> #31 -> #32`) into issue numbers.
fn parse_order(order: Option<&str>) -> Vec<u64> {
    let Some(s) = order else {
        return Vec::new();
    };
    s.split("->")
        .filter_map(|tok| {
            tok.trim()
                .trim_start_matches('#')
                .trim()
                .parse::<u64>()
                .ok()
        })
        .collect()
}

/// Read the issue numbers out of a Debug-formatted `Vec<u64>` like `[30, 18]`
/// (the runner's `human_blockers` field), tolerating `[]`/absent as empty. Each
/// run of ASCII digits is one number, so the bracket/comma framing is ignored.
fn parse_u64_list(raw: Option<&str>) -> Vec<u64> {
    let Some(s) = raw else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            cur.push(ch);
        } else if !cur.is_empty() {
            if let Ok(n) = cur.parse() {
                out.push(n);
            }
            cur.clear();
        }
    }
    if let Ok(n) = cur.parse() {
        out.push(n);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_u64_list_reads_debug_vec_and_tolerates_empty() {
        assert_eq!(parse_u64_list(Some("[30]")), vec![30]);
        assert_eq!(parse_u64_list(Some("[30, 18]")), vec![30, 18]);
        assert!(parse_u64_list(Some("[]")).is_empty());
        assert!(parse_u64_list(None).is_empty());
    }

    fn decode(fields: EventFields) -> Option<RunEvent> {
        event_to_runevent("ralphy_core::runner", &fields.message.clone(), &fields)
    }

    #[test]
    fn decoder_maps_the_idle_reap_from_either_execution_path() {
        // The normalization this pins (docs/adr/0038): the PTY driver and the
        // headless driver measure progress differently, but both emit the SAME
        // shared constant, so one decoder arm serves both and the operator gets
        // one event shape regardless of which child shape ran.
        assert_eq!(
            decode(EventFields {
                message: ralphy_adapter_support::IDLE_REAPED_MSG.into(),
                idle_minutes: Some(20),
                ..Default::default()
            }),
            Some(RunEvent::IdleReaped { idle_minutes: 20 })
        );
        assert_eq!(
            decode(EventFields {
                message: ralphy_adapter_support::IDLE_REAPED_MSG.into(),
                idle_minutes: Some(45),
                ..Default::default()
            }),
            Some(RunEvent::IdleReaped { idle_minutes: 45 })
        );
    }

    #[test]
    fn the_idle_reap_is_emitted_below_warn_so_it_stays_a_first_class_event() {
        // The decoder short-circuits WARN/ERROR into a generic `Notice`. A reap
        // logged at WARN would therefore lose its identity — no `IdleReaped`, no
        // CloudEvent, no dedicated Telegram push. This pins the level contract the
        // two emitters must honor.
        assert_eq!(
            decode(EventFields {
                message: ralphy_adapter_support::IDLE_REAPED_MSG.into(),
                idle_minutes: Some(20),
                level: Level::WARN,
                ..Default::default()
            }),
            Some(RunEvent::Notice {
                level: Level::WARN,
                message: ralphy_adapter_support::IDLE_REAPED_MSG.into(),
            }),
            "if this ever changes, the emitters may log the reap at WARN again"
        );
    }

    #[test]
    fn decoder_maps_queue_built_assignee_filter() {
        // ADR-0021 §5: a `queue built` carrying the resolved login decodes it onto
        // `QueueBuilt.assignee_filter`; a field-absent `queue built` decodes to `None`.
        assert_eq!(
            decode(EventFields {
                message: "queue built".into(),
                count: Some(1),
                order: Some("#1".into()),
                assignee_filter: Some("octocat".into()),
                ..Default::default()
            }),
            Some(RunEvent::QueueBuilt {
                count: 1,
                order: vec![1],
                stop_before: None,
                issues: serde_json::Value::Null,
                assignee_filter: Some("octocat".into()),
                scope: None,
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "queue built".into(),
                count: Some(1),
                order: Some("#1".into()),
                ..Default::default()
            }),
            Some(RunEvent::QueueBuilt {
                count: 1,
                order: vec![1],
                stop_before: None,
                issues: serde_json::Value::Null,
                assignee_filter: None,
                scope: None,
            })
        );
    }

    #[test]
    fn decoder_maps_each_consumed_info_shape() {
        assert_eq!(
            decode(EventFields {
                message: "queue built".into(),
                count: Some(3),
                order: Some("#1 -> #2 -> #3".into()),
                stop_before: Some(2),
                issues_json: Some(r#"[{"number":1,"queue_status":"eligible"}]"#.into()),
                ..Default::default()
            }),
            Some(RunEvent::QueueBuilt {
                count: 3,
                order: vec![1, 2, 3],
                stop_before: Some(2),
                issues: serde_json::json!([{"number":1,"queue_status":"eligible"}]),
                assignee_filter: None,
                scope: None,
            })
        );
        // A legacy `queue built` with no snapshot decodes with `issues: Null`.
        assert_eq!(
            decode(EventFields {
                message: "queue built".into(),
                count: Some(1),
                order: Some("#1".into()),
                ..Default::default()
            }),
            Some(RunEvent::QueueBuilt {
                count: 1,
                order: vec![1],
                stop_before: None,
                issues: serde_json::Value::Null,
                assignee_filter: None,
                scope: None,
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "issue started".into(),
                number: Some(7),
                title: Some("hello".into()),
                ..Default::default()
            }),
            Some(RunEvent::IssueStarted {
                number: 7,
                title: "hello".into()
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "plan written".into(),
                number: Some(7),
                open_steps: Some(0),
                up: Some(12_400),
                cr: Some(184_000),
                cw: Some(8_100),
                out: Some(3_200),
                model: Some("claude-opus-4".into()),
                steps_json: Some(
                    r#"[{"text":"a","status":"open"},{"text":"b","status":"checked"}]"#.into(),
                ),
                ..Default::default()
            }),
            Some(RunEvent::PlanWritten {
                number: 7,
                open_steps: 0,
                usage: UsageLite {
                    input: 12_400,
                    cache_read: 184_000,
                    cache_creation: 8_100,
                    output: 3_200,
                    model: Some("claude-opus-4".into()),
                },
                steps: vec![("a".into(), "open".into()), ("b".into(), "checked".into()),],
            })
        );
        // The adapter's planning event seeds the planning spinner's model/effort.
        assert_eq!(
            decode(EventFields {
                message: ralphy_core::emit::PLANNING_MSG.into(),
                model: Some("opus".into()),
                effort: Some("high".into()),
                ..Default::default()
            }),
            Some(RunEvent::Planning {
                model: Some("opus".into()),
                effort: Some("high".into()),
            })
        );
        assert_eq!(
            decode(EventFields {
                message: ralphy_core::emit::EXECUTING_MSG.into(),
                budget_min: Some(45),
                model: Some("claude-sonnet-4".into()),
                effort: Some("medium".into()),
                ..Default::default()
            }),
            Some(RunEvent::Executing {
                number: 0,
                budget_min: 45,
                model: "claude-sonnet-4".into(),
                effort: Some("medium".into()),
            })
        );
        assert_eq!(
            decode(EventFields {
                message: ralphy_core::emit::EXECUTING_MSG.into(),
                budget_min: Some(30),
                ..Default::default()
            }),
            Some(RunEvent::Executing {
                number: 0,
                budget_min: 30,
                model: String::new(),
                effort: None,
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "green — issue closed".into(),
                number: Some(7),
                tokens: Some(1_200_000),
                invocations: Some(3),
                up: Some(41_200),
                cr: Some(902_000),
                cw: Some(22_000),
                out: Some(18_400),
                model: Some("claude-sonnet-4".into()),
                ..Default::default()
            }),
            Some(RunEvent::IssueClosed {
                number: 7,
                tokens: 1_200_000,
                invocations: 3,
                usage: UsageLite {
                    input: 41_200,
                    cache_read: 902_000,
                    cache_creation: 22_000,
                    output: 18_400,
                    model: Some("claude-sonnet-4".into()),
                },
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "non-green — stopping run".into(),
                number: Some(7),
                outcome: Some("Stuck".into()),
                ..Default::default()
            }),
            Some(RunEvent::NonGreen {
                number: 7,
                outcome: "Stuck".into()
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "blocked by open issue(s) — skipping".into(),
                number: Some(7),
                ..Default::default()
            }),
            Some(RunEvent::Skipped {
                number: 7,
                kind: SkipKind::BlockedBy,
                label: None,
                blockers: vec![],
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "stop-before label — halting run before this issue".into(),
                number: Some(8),
                ..Default::default()
            }),
            Some(RunEvent::Skipped {
                number: 8,
                kind: SkipKind::StopBefore,
                label: None,
                blockers: vec![],
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "human-return label — skipping issue".into(),
                number: Some(9),
                label: Some("needs-info".into()),
                ..Default::default()
            }),
            Some(RunEvent::Skipped {
                number: 9,
                kind: SkipKind::HumanReturn,
                label: Some("needs-info".into()),
                blockers: vec![],
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "blocked — waiting on human".into(),
                number: Some(16),
                human_blockers: Some("[30]".into()),
                ..Default::default()
            }),
            Some(RunEvent::HumanBlocked {
                number: 16,
                on: vec![30]
            })
        );
    }

    #[test]
    fn decoder_maps_kimi_planning_and_executing() {
        // The kimi adapter's tracing strings must flip the active issue's phase
        // exactly like the other adapters; without them the live line, the
        // Telegram card, and the heartbeat phase all stay stuck on "planning".
        assert_eq!(
            decode(EventFields {
                message: ralphy_core::emit::PLANNING_MSG.into(),
                model: Some("kimi-code".into()),
                ..Default::default()
            }),
            Some(RunEvent::Planning {
                model: Some("kimi-code".into()),
                effort: None,
            })
        );
        assert_eq!(
            decode(EventFields {
                message: ralphy_core::emit::EXECUTING_MSG.into(),
                budget_min: Some(30),
                model: Some("kimi-code".into()),
                ..Default::default()
            }),
            Some(RunEvent::Executing {
                number: 0,
                budget_min: 30,
                model: "kimi-code".into(),
                effort: None,
            })
        );
    }

    #[test]
    fn decoder_reads_blocked_by_blockers_and_tolerates_absence() {
        // A dependency skip carrying the open-blocker list decodes it onto
        // `Skipped.blockers`; an absent `blockers` field decodes to an empty vec.
        assert_eq!(
            decode(EventFields {
                message: "blocked by open issue(s) — skipping".into(),
                number: Some(140),
                blockers: Some("[139]".into()),
                ..Default::default()
            }),
            Some(RunEvent::Skipped {
                number: 140,
                kind: SkipKind::BlockedBy,
                label: None,
                blockers: vec![139],
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "blocked by open issue(s) — skipping".into(),
                number: Some(140),
                ..Default::default()
            }),
            Some(RunEvent::Skipped {
                number: 140,
                kind: SkipKind::BlockedBy,
                label: None,
                blockers: vec![],
            })
        );
    }

    #[test]
    fn decoder_maps_api_degraded_events() {
        assert_eq!(
            decode(EventFields {
                message: ralphy_adapter_support::API_DEGRADED_MSG.into(),
                ..Default::default()
            }),
            Some(RunEvent::ApiDegraded)
        );
        assert_eq!(
            decode(EventFields {
                message: ralphy_adapter_support::API_RECOVERED_MSG.into(),
                ..Default::default()
            }),
            Some(RunEvent::ApiRecovered)
        );
    }

    #[test]
    fn decoder_maps_the_api_degraded_from_either_execution_path() {
        // The normalization this pins (issue #217): the PTY driver (#149) and the
        // headless driver both emit the SAME shared constant, so one decoder arm
        // serves both and the operator gets one event shape regardless of which
        // child shape ran (mold of `decoder_maps_the_idle_reap_from_either_execution_path`).
        assert_eq!(
            decode(EventFields {
                message: ralphy_adapter_support::API_DEGRADED_MSG.into(),
                ..Default::default()
            }),
            Some(RunEvent::ApiDegraded)
        );
        assert_eq!(
            decode(EventFields {
                message: ralphy_adapter_support::API_RECOVERED_MSG.into(),
                ..Default::default()
            }),
            Some(RunEvent::ApiRecovered)
        );
    }

    #[test]
    fn decoder_maps_sleep_and_deadline_events() {
        assert_eq!(
            decode(EventFields {
                message: "usage limit — waiting for reset".into(),
                reset: Some("14:30".into()),
                target_epoch: Some(1_700_000_000),
                ..Default::default()
            }),
            Some(RunEvent::SleepStarted {
                reset: "14:30".into(),
                target_epoch: 1_700_000_000
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "reset reached — resuming".into(),
                ..Default::default()
            }),
            Some(RunEvent::SleepEnded)
        );
        assert_eq!(
            decode(EventFields {
                message: "deadline passed — not starting issue".into(),
                number: Some(7),
                ..Default::default()
            }),
            Some(RunEvent::DeadlinePassed { number: 7 })
        );
    }

    #[test]
    fn decoder_level_wins_warn_and_error_emit_notice() {
        // WARN: level wins even when message matches a known INFO shape.
        let result = decode(EventFields {
            level: Level::WARN,
            message: "queue built".into(),
            count: Some(3),
            order: Some("#1 -> #2 -> #3".into()),
            ..Default::default()
        });
        assert_eq!(
            result,
            Some(RunEvent::Notice {
                level: Level::WARN,
                message: "queue built".into()
            })
        );
        // ERROR: same treatment.
        let result = decode(EventFields {
            level: Level::ERROR,
            message: "something bad happened".into(),
            ..Default::default()
        });
        assert_eq!(
            result,
            Some(RunEvent::Notice {
                level: Level::ERROR,
                message: "something bad happened".into()
            })
        );
    }

    #[test]
    fn decoder_maps_knowledge_consolidation_events() {
        assert_eq!(
            decode(EventFields {
                message: "consolidating knowledge".into(),
                count: Some(4),
                ..Default::default()
            }),
            Some(RunEvent::KnowledgeConsolidating { notes: 4 })
        );
        assert_eq!(
            decode(EventFields {
                message: "knowledge consolidated".into(),
                count: Some(4),
                ..Default::default()
            }),
            Some(RunEvent::KnowledgeConsolidated { archived: 4 })
        );
    }

    #[test]
    fn decoder_maps_run_boundary_events() {
        // `run started`: the CLI-only parameters decode into the typed variant, and
        // a `0.0` deadline sentinel folds back to `None`.
        assert_eq!(
            decode(EventFields {
                message: "run started".into(),
                repo: Some("o/r".into()),
                queue_labels: Some("AFK, ready".into()),
                agent: Some("claude".into()),
                plan_agent: Some("claude".into()),
                branch_mode: Some("new".into()),
                base: Some("origin/main".into()),
                deadline_hours: Some(0.0),
                ..Default::default()
            }),
            Some(RunEvent::RunStarted {
                repo: "o/r".into(),
                queue_labels: vec!["AFK".into(), "ready".into()],
                agent: "claude".into(),
                plan_agent: "claude".into(),
                branch_mode: "new".into(),
                branch: "origin/main".into(),
                deadline_hours: None,
            })
        );
        // A non-zero deadline survives.
        let decoded = decode(EventFields {
            message: "run started".into(),
            deadline_hours: Some(6.0),
            ..Default::default()
        });
        assert!(matches!(
            decoded,
            Some(RunEvent::RunStarted { deadline_hours: Some(h), .. }) if (h - 6.0).abs() < 1e-9
        ));

        // `run finished`: outcome + totals decode into the typed variant.
        assert_eq!(
            decode(EventFields {
                message: "run finished".into(),
                outcome: Some("completed".into()),
                issues_done: Some(3),
                issues_skipped: Some(1),
                issues_total: Some(5),
                up: Some(100),
                cr: Some(200),
                cw: Some(50),
                out: Some(25),
                duration_s: Some(412),
                ..Default::default()
            }),
            Some(RunEvent::RunFinished {
                outcome: "completed".into(),
                issues_done: 3,
                issues_skipped: 1,
                issues_total: 5,
                issues_blocked: 0,
                issues_hitl: 0,
                issues: serde_json::Value::Null,
                up: 100,
                cr: 200,
                cw: 50,
                out: 25,
                duration_s: 412,
            })
        );
    }

    #[test]
    fn decoder_maps_plan_snapshot_events_and_apply_is_noop() {
        // `plan opened`/`plan closed` decode into the raw-snapshot variants carrying
        // the plan_md field (arriving via record_str here).
        assert_eq!(
            decode(EventFields {
                message: "plan opened".into(),
                number: Some(7),
                plan_md: Some("# Plan\n## Steps\n- [ ] a\n".into()),
                ..Default::default()
            }),
            Some(RunEvent::PlanOpened {
                number: 7,
                plan_md: "# Plan\n## Steps\n- [ ] a\n".into(),
            })
        );
        assert_eq!(
            decode(EventFields {
                message: "plan closed".into(),
                number: Some(7),
                plan_md: Some("# Plan\n- [x] a\n".into()),
                ..Default::default()
            }),
            Some(RunEvent::PlanClosed {
                number: 7,
                plan_md: "# Plan\n- [x] a\n".into(),
            })
        );
        // Folding either snapshot is a no-op on the card model.
        let mut before = crate::runstate::RunState::new("t", 1);
        before.apply(RunEvent::IssueStarted {
            number: 7,
            title: "a".into(),
        });
        let mut after = before.clone();
        after.apply(RunEvent::PlanOpened {
            number: 7,
            plan_md: "x".into(),
        });
        after.apply(RunEvent::PlanClosed {
            number: 7,
            plan_md: "y".into(),
        });
        assert_eq!(before, after);
    }

    #[test]
    fn parse_steps_json_maps_array_and_tolerates_absence() {
        assert_eq!(
            parse_steps_json(Some(
                r#"[{"text":"a","status":"open"},{"text":"b","status":"checked"}]"#
            )),
            vec![("a".into(), "open".into()), ("b".into(), "checked".into())]
        );
        assert!(parse_steps_json(None).is_empty());
        assert!(parse_steps_json(Some("not json")).is_empty());
    }

    #[test]
    fn decoder_unknown_info_message_returns_none() {
        assert_eq!(
            decode(EventFields {
                message: "some unrelated log line".into(),
                ..Default::default()
            }),
            None
        );
    }
}
