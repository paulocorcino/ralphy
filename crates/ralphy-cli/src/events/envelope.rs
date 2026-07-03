//! The `RunEvent` -> CloudEvents 1.0 envelope mapping (ADR-0019).
//!
//! [`runevent_to_cloudevent`] is the single pure function that turns one folded
//! [`RunEvent`] into a CloudEvents 1.0 structured-mode JSON envelope, matching the
//! `docs/events.md` catalog. It is unit-tested per variant so a drift between an
//! event and its wire shape fails a test rather than silently changing the
//! contract — the per-variant tests are the source of truth the doc defers to.

use serde_json::{json, Value};

use crate::runstate::{RunEvent, RunState, SkipKind, UsageLite};

/// The per-run context every envelope shares: the `source` (`ralphy/<owner>/<repo>`),
/// the `runid` correlation ULID minted at process start, and the pre-serialized
/// `data.emitter` identity object.
#[derive(Debug, Clone)]
pub struct EventCtx {
    pub source: String,
    pub runid: String,
    pub emitter: Value,
}

/// Assemble a CloudEvents 1.0 structured-mode envelope. `data` is the
/// event-specific object; the reserved `emitter` identity is merged into it, and
/// the envelope carries exactly one extension attribute — `runid` (ADR-0019 §3).
fn envelope(type_: &str, subject: Option<&str>, ctx: &EventCtx, data: Value) -> Value {
    let mut data = data;
    if let Value::Object(ref mut map) = data {
        map.insert("emitter".to_string(), ctx.emitter.clone());
    }
    let mut ev = serde_json::Map::new();
    ev.insert("specversion".to_string(), json!("1.0"));
    ev.insert("type".to_string(), json!(type_));
    ev.insert("source".to_string(), json!(ctx.source));
    if let Some(subject) = subject {
        ev.insert("subject".to_string(), json!(subject));
    }
    ev.insert("id".to_string(), json!(super::emitter::new_id()));
    ev.insert(
        "time".to_string(),
        json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
    );
    ev.insert("runid".to_string(), json!(ctx.runid));
    ev.insert("datacontenttype".to_string(), json!("application/json"));
    ev.insert("data".to_string(), data);
    Value::Object(ev)
}

/// Build a subject-less envelope for a synthetic `run.*` event that has no
/// [`RunEvent`] source (the sink's own `run.heartbeat`). Shares the exact envelope
/// assembly — specversion, id, time, runid, merged emitter — as the mapped events.
pub fn run_envelope(type_: &str, ctx: &EventCtx, data: Value) -> Value {
    envelope(type_, None, ctx, data)
}

/// The `data` payload shared by the enriched `queue.built` and the on-demand
/// `queue.snapshot` (ADR-0020): `{count, order, stop_before, issues}`. Defined
/// ONCE and used by both triggers so the two payloads can never diverge — `issues`
/// is the per-issue snapshot array (`Value::Null` for the legacy shape when the
/// resolver produced none). The reserved `emitter` block is merged in later by
/// [`envelope`]/[`run_envelope`].
pub fn queue_snapshot_data(
    issues: &Value,
    count: u64,
    order: &[u64],
    stop_before: Option<u64>,
) -> Value {
    json!({
        "count": count,
        "order": order,
        "stop_before": stop_before,
        "issues": issues,
    })
}

/// Wrap a [`queue_snapshot_data`] payload in the `dev.ralphy.queue.snapshot`
/// envelope (ADR-0020): the on-demand `ralphy issues --push` emission, byte-for-byte
/// the same `data` shape the runner's `queue.built` carries, only the envelope
/// `type` differs. Subject-less, like `queue.built`.
pub fn queue_snapshot_envelope(data: Value, ctx: &EventCtx) -> Value {
    run_envelope("dev.ralphy.queue.snapshot", ctx, data)
}

/// The `usage {up,cr,cw,out,model}` object carried on `plan.written` and
/// `issue.closed` (docs/events.md); `model` is `null` when the adapter captured none.
fn usage_json(u: &UsageLite) -> Value {
    json!({
        "up": u.input,
        "cr": u.cache_read,
        "cw": u.cache_creation,
        "out": u.output,
        "model": u.model,
    })
}

/// The `issue/<n>` subject carried on every `issue.*` event and on `plan.written`.
fn subject_for(n: u64) -> String {
    format!("issue/{n}")
}

/// The [`SkipKind`] wire name on an `issue.skipped` event (docs/events.md).
fn skip_kind_name(kind: SkipKind) -> &'static str {
    match kind {
        SkipKind::BlockedBy => "blocked_by",
        SkipKind::StopBefore => "stop_before",
        SkipKind::HumanReturn => "human_return",
        SkipKind::VerifyFailed => "verify_failed",
    }
}

/// Resolve a possibly-zero issue number (the adapter's planning/execution events
/// carry no number) to the active issue, mirroring `RunState::resolve`.
fn resolve(state: &RunState, number: u64) -> Option<u64> {
    if number == 0 {
        state.active
    } else {
        Some(number)
    }
}

/// Map one folded [`RunEvent`] to a CloudEvents envelope, or `None` for an event
/// the sink does not forward. `state` resolves the active issue number for the
/// adapter events that carry `0` (planning/executing), mirroring the notifier fold.
///
/// Pure over `(ev, ctx, state)` apart from the per-event ULID `id` and UTC `time`
/// the envelope stamps — those are asserted for presence/shape, not equality.
pub fn runevent_to_cloudevent(ev: &RunEvent, ctx: &EventCtx, state: &RunState) -> Option<Value> {
    match ev {
        RunEvent::QueueBuilt {
            count,
            order,
            stop_before,
            issues,
        } => Some(envelope(
            "dev.ralphy.queue.built",
            None,
            ctx,
            queue_snapshot_data(issues, *count, order, *stop_before),
        )),
        RunEvent::IssueStarted { number, title } => Some(envelope(
            "dev.ralphy.issue.started",
            Some(&subject_for(*number)),
            ctx,
            json!({ "number": number, "title": title }),
        )),
        RunEvent::Planning { model, effort } => {
            // The adapter event carries no number; the subject is the active issue.
            let subject = state.active.map(subject_for);
            Some(envelope(
                "dev.ralphy.issue.planning",
                subject.as_deref(),
                ctx,
                json!({ "model": model, "effort": effort }),
            ))
        }
        RunEvent::PlanWritten {
            number,
            open_steps,
            usage,
        } => Some(envelope(
            "dev.ralphy.plan.written",
            Some(&subject_for(*number)),
            ctx,
            json!({ "number": number, "open_steps": open_steps, "usage": usage_json(usage) }),
        )),
        RunEvent::Executing {
            number,
            budget_min,
            model,
            effort,
        } => {
            let n = resolve(state, *number);
            let subject = n.map(subject_for);
            Some(envelope(
                "dev.ralphy.issue.executing",
                subject.as_deref(),
                ctx,
                json!({
                    "number": n.unwrap_or(0),
                    "budget_min": budget_min,
                    "model": model,
                    "effort": effort,
                }),
            ))
        }
        RunEvent::IssueClosed {
            number,
            tokens,
            usage,
        } => Some(envelope(
            "dev.ralphy.issue.closed",
            Some(&subject_for(*number)),
            ctx,
            json!({ "number": number, "tokens": tokens, "usage": usage_json(usage) }),
        )),
        RunEvent::NonGreen { number, outcome } => Some(envelope(
            "dev.ralphy.issue.non_green",
            Some(&subject_for(*number)),
            ctx,
            json!({ "number": number, "outcome": outcome }),
        )),
        RunEvent::NeedsSplit { number } => Some(envelope(
            "dev.ralphy.issue.needs_split",
            Some(&subject_for(*number)),
            ctx,
            json!({ "number": number }),
        )),
        RunEvent::Skipped {
            number,
            kind,
            label,
        } => Some(envelope(
            "dev.ralphy.issue.skipped",
            Some(&subject_for(*number)),
            ctx,
            json!({ "number": number, "kind": skip_kind_name(*kind), "label": label }),
        )),
        RunEvent::HumanBlocked { number, on } => Some(envelope(
            "dev.ralphy.issue.human_blocked",
            Some(&subject_for(*number)),
            ctx,
            json!({ "number": number, "on": on }),
        )),
        RunEvent::DeadlinePassed { number } => Some(envelope(
            "dev.ralphy.issue.deadline_passed",
            Some(&subject_for(*number)),
            ctx,
            json!({ "number": number }),
        )),
        RunEvent::SleepStarted {
            reset,
            target_epoch,
        } => Some(envelope(
            "dev.ralphy.run.sleep_started",
            None,
            ctx,
            json!({ "reset": reset, "target_epoch": target_epoch }),
        )),
        RunEvent::SleepEnded => Some(envelope("dev.ralphy.run.sleep_ended", None, ctx, json!({}))),
        RunEvent::KnowledgeConsolidating { notes } => Some(envelope(
            "dev.ralphy.knowledge.consolidating",
            None,
            ctx,
            json!({ "notes": notes }),
        )),
        RunEvent::KnowledgeConsolidated { archived } => Some(envelope(
            "dev.ralphy.knowledge.consolidated",
            None,
            ctx,
            json!({ "archived": archived }),
        )),
        RunEvent::Notice { level, message } => Some(envelope(
            "dev.ralphy.run.notice",
            None,
            ctx,
            json!({ "level": level.to_string().to_lowercase(), "message": message }),
        )),
        RunEvent::RunStarted {
            repo,
            queue_labels,
            agent,
            plan_agent,
            branch_mode,
            branch,
            deadline_hours,
        } => Some(envelope(
            "dev.ralphy.run.started",
            None,
            ctx,
            json!({
                "repo": repo,
                "queue_labels": queue_labels,
                "agent": agent,
                "plan_agent": plan_agent,
                "branch_mode": branch_mode,
                "branch": branch,
                "deadline_hours": deadline_hours,
            }),
        )),
        RunEvent::RunFinished {
            outcome,
            issues_done,
            issues_skipped,
            issues_total,
            up,
            cr,
            cw,
            out,
            duration_s,
        } => Some(envelope(
            "dev.ralphy.run.finished",
            None,
            ctx,
            json!({
                "outcome": outcome,
                "issues_done": issues_done,
                "issues_skipped": issues_skipped,
                "issues_total": issues_total,
                "tokens_total": { "up": up, "cr": cr, "cw": cw, "out": out },
                "duration_s": duration_s,
            }),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test context with a stub emitter object.
    fn ctx() -> EventCtx {
        EventCtx {
            source: "ralphy/o/r".to_string(),
            runid: "01RUNIDRUNIDRUNIDRUNID".to_string(),
            emitter: json!({ "version": "0.0.0", "pid": 4242 }),
        }
    }

    #[test]
    fn issue_closed_has_full_envelope_shape() {
        let ev = RunEvent::IssueClosed {
            number: 7,
            tokens: 42,
            usage: UsageLite {
                input: 1,
                cache_read: 2,
                cache_creation: 3,
                output: 4,
                model: Some("claude-sonnet-4".into()),
            },
        };
        let v = runevent_to_cloudevent(&ev, &ctx(), &RunState::new("t", 1)).unwrap();
        assert_eq!(v["specversion"], "1.0");
        assert_eq!(v["type"], "dev.ralphy.issue.closed");
        assert_eq!(v["source"], "ralphy/o/r");
        assert_eq!(v["subject"], "issue/7");
        assert_eq!(v["datacontenttype"], "application/json");
        assert_eq!(v["runid"], "01RUNIDRUNIDRUNIDRUNID");
        // The per-event id and time are present and well-shaped.
        assert!(v["id"].as_str().is_some_and(|s| !s.is_empty()), "id: {v}");
        assert!(
            v["time"].as_str().is_some_and(|s| s.ends_with('Z')),
            "time not UTC: {v}"
        );
        // Data fields + the reserved emitter block merged in.
        assert_eq!(v["data"]["number"], 7);
        assert_eq!(v["data"]["tokens"], 42);
        assert_eq!(v["data"]["usage"]["up"], 1);
        assert_eq!(v["data"]["usage"]["out"], 4);
        assert_eq!(v["data"]["usage"]["model"], "claude-sonnet-4");
        assert_eq!(v["data"]["emitter"]["pid"], 4242);
    }

    /// A folded state with issue 7 active (so the number-0 adapter events resolve).
    fn active_state() -> RunState {
        let mut s = RunState::new("t", 1);
        s.apply(RunEvent::IssueStarted {
            number: 7,
            title: "hello".into(),
        });
        s
    }

    fn map(ev: RunEvent, state: &RunState) -> Value {
        runevent_to_cloudevent(&ev, &ctx(), state).expect("mapped")
    }

    #[test]
    fn queue_built_has_no_subject_and_lists_order() {
        let v = map(
            RunEvent::QueueBuilt {
                count: 3,
                order: vec![1, 2, 3],
                stop_before: Some(2),
                issues: Value::Null,
            },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.queue.built");
        assert!(
            v.get("subject").is_none(),
            "queue.built has no subject: {v}"
        );
        assert_eq!(v["data"]["count"], 3);
        assert_eq!(v["data"]["order"], json!([1, 2, 3]));
        assert_eq!(v["data"]["stop_before"], 2);
    }

    #[test]
    fn queue_built_carries_the_enriched_issues_array() {
        // The enriched `queue.built` (ADR-0020) carries the per-issue snapshot
        // verbatim under `data.issues`.
        let issues = json!([
            {"number": 1, "title": "a", "labels": ["q"], "queue_status": "eligible",
             "skip_reason": null, "blocked_by": [], "position": 1},
            {"number": 2, "title": "b", "labels": ["q"], "queue_status": "blocked",
             "skip_reason": null, "blocked_by": [1], "position": null},
        ]);
        let v = map(
            RunEvent::QueueBuilt {
                count: 2,
                order: vec![1, 2],
                stop_before: None,
                issues: issues.clone(),
            },
            &RunState::new("t", 1),
        );
        assert_eq!(v["data"]["issues"], issues);
        assert_eq!(v["data"]["count"], 2);
        assert_eq!(v["data"]["order"], json!([1, 2]));
    }

    #[test]
    fn queue_snapshot_data_matches_queue_built_data() {
        // `queue.snapshot` (from `ralphy issues --push`) and the enriched
        // `queue.built` share ONE `data` builder, so their payloads are identical
        // — only the envelope `type` differs (ADR-0020).
        let issues = json!([
            {"number": 1, "queue_status": "eligible", "position": 1},
        ]);
        let built = map(
            RunEvent::QueueBuilt {
                count: 1,
                order: vec![1],
                stop_before: None,
                issues: issues.clone(),
            },
            &RunState::new("t", 1),
        );
        let snapshot = queue_snapshot_envelope(queue_snapshot_data(&issues, 1, &[1], None), &ctx());
        assert_eq!(snapshot["type"], "dev.ralphy.queue.snapshot");
        assert!(
            snapshot.get("subject").is_none(),
            "queue.snapshot has no subject: {snapshot}"
        );
        // Byte-identical `data` shape (both merge the same emitter via ctx()).
        assert_eq!(snapshot["data"], built["data"]);
    }

    #[test]
    fn issue_started_carries_number_title_and_subject() {
        let v = map(
            RunEvent::IssueStarted {
                number: 7,
                title: "hello".into(),
            },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.issue.started");
        assert_eq!(v["subject"], "issue/7");
        assert_eq!(v["data"]["number"], 7);
        assert_eq!(v["data"]["title"], "hello");
    }

    #[test]
    fn planning_resolves_subject_from_active() {
        let v = map(
            RunEvent::Planning {
                model: Some("opus".into()),
                effort: Some("high".into()),
            },
            &active_state(),
        );
        assert_eq!(v["type"], "dev.ralphy.issue.planning");
        assert_eq!(v["subject"], "issue/7");
        assert_eq!(v["data"]["model"], "opus");
        assert_eq!(v["data"]["effort"], "high");
    }

    #[test]
    fn plan_written_carries_open_steps_usage_and_subject() {
        let v = map(
            RunEvent::PlanWritten {
                number: 7,
                open_steps: 4,
                usage: UsageLite {
                    input: 10,
                    ..Default::default()
                },
            },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.plan.written");
        assert_eq!(v["subject"], "issue/7");
        assert_eq!(v["data"]["open_steps"], 4);
        assert_eq!(v["data"]["usage"]["up"], 10);
    }

    #[test]
    fn executing_resolves_active_number_and_subject() {
        let v = map(
            RunEvent::Executing {
                number: 0,
                budget_min: 45,
                model: "claude-sonnet-4".into(),
                effort: Some("medium".into()),
            },
            &active_state(),
        );
        assert_eq!(v["type"], "dev.ralphy.issue.executing");
        assert_eq!(v["subject"], "issue/7");
        assert_eq!(v["data"]["number"], 7);
        assert_eq!(v["data"]["budget_min"], 45);
        assert_eq!(v["data"]["model"], "claude-sonnet-4");
        assert_eq!(v["data"]["effort"], "medium");
    }

    #[test]
    fn non_green_carries_outcome_and_subject() {
        let v = map(
            RunEvent::NonGreen {
                number: 7,
                outcome: "Stuck".into(),
            },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.issue.non_green");
        assert_eq!(v["subject"], "issue/7");
        assert_eq!(v["data"]["outcome"], "Stuck");
    }

    #[test]
    fn needs_split_carries_number_and_subject() {
        let v = map(RunEvent::NeedsSplit { number: 7 }, &RunState::new("t", 1));
        assert_eq!(v["type"], "dev.ralphy.issue.needs_split");
        assert_eq!(v["subject"], "issue/7");
        assert_eq!(v["data"]["number"], 7);
    }

    #[test]
    fn skipped_maps_kind_and_parking_label() {
        // A human-return skip names the parking label.
        let v = map(
            RunEvent::Skipped {
                number: 9,
                kind: SkipKind::HumanReturn,
                label: Some("needs-info".into()),
            },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.issue.skipped");
        assert_eq!(v["subject"], "issue/9");
        assert_eq!(v["data"]["kind"], "human_return");
        assert_eq!(v["data"]["label"], "needs-info");

        // A blocked-by skip has no parking label and the `blocked_by` kind.
        let v = map(
            RunEvent::Skipped {
                number: 4,
                kind: SkipKind::BlockedBy,
                label: None,
            },
            &RunState::new("t", 1),
        );
        assert_eq!(v["data"]["kind"], "blocked_by");
        assert!(v["data"]["label"].is_null(), "no parking label: {v}");
    }

    #[test]
    fn human_blocked_lists_blockers() {
        let v = map(
            RunEvent::HumanBlocked {
                number: 16,
                on: vec![30, 18],
            },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.issue.human_blocked");
        assert_eq!(v["subject"], "issue/16");
        assert_eq!(v["data"]["on"], json!([30, 18]));
    }

    #[test]
    fn deadline_passed_carries_number_and_subject() {
        let v = map(
            RunEvent::DeadlinePassed { number: 7 },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.issue.deadline_passed");
        assert_eq!(v["subject"], "issue/7");
    }

    #[test]
    fn sleep_events_map_without_subject() {
        let v = map(
            RunEvent::SleepStarted {
                reset: "14:30".into(),
                target_epoch: 1_700_000_000,
            },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.run.sleep_started");
        assert!(
            v.get("subject").is_none(),
            "sleep_started has no subject: {v}"
        );
        assert_eq!(v["data"]["reset"], "14:30");
        assert_eq!(v["data"]["target_epoch"], 1_700_000_000i64);

        let v = map(RunEvent::SleepEnded, &RunState::new("t", 1));
        assert_eq!(v["type"], "dev.ralphy.run.sleep_ended");
        assert!(
            v.get("subject").is_none(),
            "sleep_ended has no subject: {v}"
        );
    }

    #[test]
    fn knowledge_events_map_counts() {
        let v = map(
            RunEvent::KnowledgeConsolidating { notes: 4 },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.knowledge.consolidating");
        assert_eq!(v["data"]["notes"], 4);

        let v = map(
            RunEvent::KnowledgeConsolidated { archived: 3 },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.knowledge.consolidated");
        assert_eq!(v["data"]["archived"], 3);
    }

    #[test]
    fn run_started_maps_cli_params_without_subject() {
        let v = map(
            RunEvent::RunStarted {
                repo: "o/r".into(),
                queue_labels: vec!["AFK".into(), "ready".into()],
                agent: "claude".into(),
                plan_agent: "codex".into(),
                branch_mode: "new".into(),
                branch: "origin/main".into(),
                deadline_hours: Some(6.0),
            },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.run.started");
        assert!(
            v.get("subject").is_none(),
            "run.started has no subject: {v}"
        );
        assert_eq!(v["data"]["repo"], "o/r");
        assert_eq!(v["data"]["queue_labels"], json!(["AFK", "ready"]));
        assert_eq!(v["data"]["agent"], "claude");
        assert_eq!(v["data"]["plan_agent"], "codex");
        assert_eq!(v["data"]["branch_mode"], "new");
        assert_eq!(v["data"]["branch"], "origin/main");
        assert_eq!(v["data"]["deadline_hours"], 6.0);
    }

    #[test]
    fn run_finished_maps_outcome_totals_without_subject() {
        let v = map(
            RunEvent::RunFinished {
                outcome: "completed".into(),
                issues_done: 3,
                issues_skipped: 1,
                issues_total: 5,
                up: 100,
                cr: 200,
                cw: 50,
                out: 25,
                duration_s: 412,
            },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.run.finished");
        assert!(
            v.get("subject").is_none(),
            "run.finished has no subject: {v}"
        );
        assert_eq!(v["data"]["outcome"], "completed");
        assert_eq!(v["data"]["issues_done"], 3);
        assert_eq!(v["data"]["issues_skipped"], 1);
        assert_eq!(v["data"]["issues_total"], 5);
        assert_eq!(v["data"]["tokens_total"]["up"], 100);
        assert_eq!(v["data"]["tokens_total"]["out"], 25);
        assert_eq!(v["data"]["duration_s"], 412);
    }

    #[test]
    fn notice_maps_level_and_message_without_subject() {
        let v = map(
            RunEvent::Notice {
                level: tracing::Level::WARN,
                message: "heads up".into(),
            },
            &RunState::new("t", 1),
        );
        assert_eq!(v["type"], "dev.ralphy.run.notice");
        assert!(v.get("subject").is_none(), "notice carries no subject: {v}");
        assert_eq!(v["data"]["level"], "warn");
        assert_eq!(v["data"]["message"], "heads up");
    }
}
