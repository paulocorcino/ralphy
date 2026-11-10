//! The `RunEvent` -> `CloudEvents` 1.0 envelope mapping (ADR-0019). The per-variant tests
//! are the source of truth for the wire shape that `docs/events.md` describes.

use serde_json::{json, Value};

use crate::runstate::{RunEvent, RunState, SkipKind, UsageLite};

mod rollup;

/// The per-run context every envelope shares: `source` (`ralphy/<owner>/<repo>`), the
/// `runid` ULID minted at process start, the `data.emitter` object and the `data.git` block.
#[derive(Debug, Clone)]
pub(crate) struct EventCtx {
    pub source: String,
    pub runid: String,
    pub emitter: Value,
    /// The reserved `data.git` block: `{repository, branch}`, constant per run (ADR-0019).
    pub git: Value,
}

/// The reserved `data.agent` block (ADR-0019): `{name, model, effort}` of the current phase.
fn agent_block(state: &RunState) -> Value {
    let name = state
        .cur_agent
        .clone()
        .or_else(|| (!state.exec_agent.is_empty()).then(|| state.exec_agent.clone()));
    json!({ "name": name, "model": state.cur_model, "effort": state.cur_effort })
}

/// The `issue/<n>` subject's issue number (`None` for a malformed subject).
fn issue_number_from_subject(subject: &str) -> Option<u64> {
    subject.strip_prefix("issue/").and_then(|s| s.parse().ok())
}

/// The folded issue title when non-empty, else the `queue.built` seed, else empty.
fn issue_title(state: &RunState, number: u64) -> String {
    state
        .issues
        .iter()
        .find(|e| e.number == number)
        .map(|e| e.title.clone())
        .filter(|t| !t.is_empty())
        .or_else(|| {
            state
                .queue
                .iter()
                .find(|q| q.number == number)
                .map(|q| q.title.clone())
        })
        .unwrap_or_default()
}

/// The reserved `data.issue` block (ADR-0019) of every subject-scoped event.
fn issue_block(state: &RunState, number: u64) -> Value {
    json!({ "number": number, "title": issue_title(state, number) })
}

/// A `CloudEvents` 1.0 envelope with exactly one extension attribute, `runid` (ADR-0019 §3).
fn envelope(
    type_: &str,
    subject: Option<&str>,
    ctx: &EventCtx,
    state: &RunState,
    data: Value,
) -> Value {
    let mut data = data;
    if let Value::Object(ref mut map) = data {
        map.insert("emitter".to_string(), ctx.emitter.clone());
        map.insert("git".to_string(), ctx.git.clone());
        if type_ != "dev.ralphy.run.finished" {
            map.insert("agent".to_string(), agent_block(state));
        }
        if let Some(number) = subject.and_then(issue_number_from_subject) {
            map.insert("issue".to_string(), issue_block(state, number));
        }
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

/// A subject-less envelope for a synthetic `run.*` event with no [`RunEvent`] source.
pub(crate) fn run_envelope(type_: &str, ctx: &EventCtx, state: &RunState, data: Value) -> Value {
    envelope(type_, None, ctx, state, data)
}

/// A `dev.ralphy.plan.step` envelope (#96): one checkbox moves to `checked` or `noticed`.
pub(crate) fn plan_step_envelope(
    ctx: &EventCtx,
    state: &RunState,
    number: u64,
    text: &str,
    status: &str,
) -> Value {
    envelope(
        "dev.ralphy.plan.step",
        Some(&subject_for(number)),
        ctx,
        state,
        json!({ "text": text, "status": status }),
    )
}

/// The `data` of `queue.built` and `queue.snapshot` (ADR-0020), built once so they cannot
/// diverge. `assignee_filter` is the resolved login (ADR-0021 §5), `null` for the whole queue.
pub(crate) fn queue_snapshot_data(
    issues: &Value,
    count: u64,
    order: &[u64],
    stop_before: Option<u64>,
    assignee_filter: Option<&str>,
) -> Value {
    json!({
        "count": count,
        "order": order,
        "stop_before": stop_before,
        "issues": issues,
        "assignee_filter": assignee_filter,
    })
}

/// The `dev.ralphy.queue.snapshot` envelope of `ralphy issues --push` (ADR-0020).
pub(crate) fn queue_snapshot_envelope(data: Value, ctx: &EventCtx, state: &RunState) -> Value {
    run_envelope("dev.ralphy.queue.snapshot", ctx, state, data)
}

/// The `usage {up,cr,cw,out,model}` object of `plan.written` and `issue.closed`.
fn usage_json(u: &UsageLite) -> Value {
    json!({
        "up": u.input,
        "cr": u.cache_read,
        "cw": u.cache_creation,
        "out": u.output,
        "model": u.model,
    })
}

/// The `issue/<n>` subject.
fn subject_for(n: u64) -> String {
    format!("issue/{n}")
}

/// The [`SkipKind`] wire name on `issue.skipped`.
fn skip_kind_name(kind: SkipKind) -> &'static str {
    kind.skip_wire()
}

/// Resolve a zero issue number (adapter events carry none) to the active issue.
fn resolve(state: &RunState, number: u64) -> Option<u64> {
    if number == 0 {
        state.active
    } else {
        Some(number)
    }
}

/// Map one folded [`RunEvent`] to its envelope.
#[expect(
    clippy::too_many_lines,
    reason = "one flat arm per RunEvent variant, matched exhaustively; a split loses that check"
)]
pub(crate) fn runevent_to_cloudevent(ev: &RunEvent, ctx: &EventCtx, state: &RunState) -> Value {
    match ev {
        RunEvent::QueueBuilt {
            count,
            order,
            stop_before,
            issues,
            assignee_filter,
            // Log-only (#222): the scope phrase never reaches the wire.
            scope: _,
        } => envelope(
            "dev.ralphy.queue.built",
            None,
            ctx,
            state,
            queue_snapshot_data(
                issues,
                *count,
                order,
                *stop_before,
                assignee_filter.as_deref(),
            ),
        ),
        RunEvent::IssueStarted { number, title } => envelope(
            "dev.ralphy.issue.started",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number, "title": title }),
        ),
        RunEvent::Planning { model, effort } => {
            let subject = state.active.map(subject_for);
            envelope(
                "dev.ralphy.issue.planning",
                subject.as_deref(),
                ctx,
                state,
                json!({ "model": model, "effort": effort }),
            )
        }
        RunEvent::PlanWritten {
            number,
            open_steps,
            usage,
            steps,
        } => {
            let steps: Vec<Value> = steps
                .iter()
                .map(|(text, status)| json!({ "text": text, "status": status }))
                .collect();
            envelope(
                "dev.ralphy.plan.written",
                Some(&subject_for(*number)),
                ctx,
                state,
                json!({
                    "number": number,
                    "open_steps": open_steps,
                    "usage": usage_json(usage),
                    "steps": steps,
                }),
            )
        }
        RunEvent::Executing {
            number,
            budget_min,
            model,
            effort,
        } => {
            let n = resolve(state, *number);
            let subject = n.map(subject_for);
            envelope(
                "dev.ralphy.issue.executing",
                subject.as_deref(),
                ctx,
                state,
                json!({
                    "number": n.unwrap_or(0),
                    "budget_min": budget_min,
                    "model": model,
                    "effort": effort,
                }),
            )
        }
        RunEvent::IssueClosed {
            number,
            tokens,
            usage,
            // Console-only view; it never rides the envelope (ADR-0008 D8).
            invocations: _,
        } => envelope(
            "dev.ralphy.issue.closed",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number, "tokens": tokens, "usage": usage_json(usage) }),
        ),
        RunEvent::NonGreen { number, outcome } => envelope(
            "dev.ralphy.issue.non_green",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number, "outcome": outcome }),
        ),
        RunEvent::NeedsSplit { number } => envelope(
            "dev.ralphy.issue.needs_split",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number }),
        ),
        RunEvent::Skipped {
            number,
            kind,
            label,
            blockers,
        } => envelope(
            "dev.ralphy.issue.skipped",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({
                "number": number,
                "kind": skip_kind_name(*kind),
                "label": label,
                "blocked_by": blockers,
            }),
        ),
        RunEvent::HumanBlocked { number, on } => envelope(
            "dev.ralphy.issue.human_blocked",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number, "on": on }),
        ),
        RunEvent::RunSkipped { reason } => envelope(
            "dev.ralphy.run.skipped",
            None,
            ctx,
            state,
            json!({ "reason": reason }),
        ),
        RunEvent::DeadlinePassed { number } => envelope(
            "dev.ralphy.issue.deadline_passed",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number }),
        ),
        // Run-level, sent at the moment of the stop (`run.finished` lands after teardown).
        // The subject names the issue in flight; `number: 0` (between issues) becomes `null`.
        RunEvent::RunStopped { number } => envelope(
            "dev.ralphy.run.stopped",
            (*number != 0).then(|| subject_for(*number)).as_deref(),
            ctx,
            state,
            json!({ "number": (*number != 0).then_some(*number) }),
        ),
        RunEvent::SleepStarted {
            reset,
            target_epoch,
        } => envelope(
            "dev.ralphy.run.sleep_started",
            None,
            ctx,
            state,
            json!({ "reset": reset, "target_epoch": target_epoch }),
        ),
        RunEvent::SleepEnded => envelope("dev.ralphy.run.sleep_ended", None, ctx, state, json!({})),
        RunEvent::ApiDegraded => {
            envelope("dev.ralphy.run.api_degraded", None, ctx, state, json!({}))
        }
        RunEvent::ApiRecovered => {
            envelope("dev.ralphy.run.api_recovered", None, ctx, state, json!({}))
        }
        // The reap carries its window, so a consumer need not read the run's config.
        RunEvent::IdleReaped { idle_minutes } => envelope(
            "dev.ralphy.run.idle_reaped",
            None,
            ctx,
            state,
            json!({ "idle_minutes": idle_minutes }),
        ),
        // The cause is the refusal the waiver let the session escape (ADR-0072 D6).
        RunEvent::HooksUnprovenAllowed { cause } => envelope(
            "dev.ralphy.run.hooks_unproven_allowed",
            None,
            ctx,
            state,
            json!({ "cause": cause }),
        ),
        // The agent's own state (ADR-0059 §2), on the active issue's subject.
        RunEvent::AgentState {
            state: agent_state,
            since,
            detail,
        } => envelope(
            "dev.ralphy.issue.agent_state",
            state.active.map(subject_for).as_deref(),
            ctx,
            state,
            json!({
                "state": agent_state,
                "since": since,
                "detail": detail,
            }),
        ),
        RunEvent::KnowledgeConsolidating { notes } => envelope(
            "dev.ralphy.knowledge.consolidating",
            None,
            ctx,
            state,
            json!({ "notes": notes }),
        ),
        RunEvent::KnowledgeConsolidated { archived } => envelope(
            "dev.ralphy.knowledge.consolidated",
            None,
            ctx,
            state,
            json!({ "archived": archived }),
        ),
        RunEvent::Notice { level, message } => envelope(
            "dev.ralphy.run.notice",
            None,
            ctx,
            state,
            json!({ "level": level.to_string().to_lowercase(), "message": message }),
        ),
        RunEvent::RunStarted {
            repo,
            queue_labels,
            // The exec agent rides `data.agent.name`; the block holds only one name.
            agent: _,
            plan_agent,
            branch_mode,
            branch,
            deadline_hours,
        } => {
            // The light `[{number,title}]` list seeded by `queue.built` (#96).
            let queue: Vec<Value> = state
                .queue
                .iter()
                .map(|q| json!({ "number": q.number, "title": q.title }))
                .collect();
            envelope(
                "dev.ralphy.run.started",
                None,
                ctx,
                state,
                json!({
                    "repo": repo,
                    "queue_labels": queue_labels,
                    "plan_agent": plan_agent,
                    "branch_mode": branch_mode,
                    "base": branch,
                    "deadline_hours": deadline_hours,
                    "queue": queue,
                }),
            )
        }
        RunEvent::RunFinished {
            outcome,
            issues_done,
            issues_skipped,
            issues_total,
            issues_blocked,
            issues_hitl,
            issues: rollup,
            up,
            cr,
            cw,
            out,
            duration_s,
        } => {
            let issues = rollup::finished_issues(rollup, state);
            envelope(
                "dev.ralphy.run.finished",
                None,
                ctx,
                state,
                json!({
                    "outcome": outcome,
                    "issues_done": issues_done,
                    "issues_skipped": issues_skipped,
                    "issues_total": issues_total,
                    "issues_blocked": issues_blocked,
                    "issues_hitl": issues_hitl,
                    "issues": issues,
                    "tokens_total": { "up": up, "cr": cr, "cw": cw, "out": out },
                    "duration_s": duration_s,
                }),
            )
        }
        // The raw plan snapshots (#96): the complete `plan.md` under `data.plan_md`.
        RunEvent::PlanOpened { number, plan_md } => envelope(
            "dev.ralphy.plan.opened",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number, "plan_md": plan_md }),
        ),
        RunEvent::PlanClosed { number, plan_md } => envelope(
            "dev.ralphy.plan.closed",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number, "plan_md": plan_md }),
        ),
    }
}

#[cfg(test)]
mod rollup_tests;
#[cfg(test)]
mod tests;
