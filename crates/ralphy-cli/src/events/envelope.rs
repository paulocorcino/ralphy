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
/// the `runid` correlation ULID minted at process start, the pre-serialized
/// `data.emitter` identity object, and the constant-per-run `data.git` block
/// (`{repository, branch}`) resolved before the ctx is built.
#[derive(Debug, Clone)]
pub struct EventCtx {
    pub source: String,
    pub runid: String,
    pub emitter: Value,
    /// The reserved `data.git` block merged into every envelope: `{repository,
    /// branch}` — the owner/repo slug and the operating run branch commits land on.
    /// Constant per run (ADR-0019 amendment #96).
    pub git: Value,
}

/// The reserved `data.agent` contextual block (ADR-0019 amendment #96): the current
/// phase's `{name, model, effort}`. `name` is the current phase agent (plan agent
/// while planning, exec agent while executing), falling back to the run's exec agent
/// before any phase begins, or `null` when `run.started` has not been folded yet
/// (e.g. on `queue.built`). `model`/`effort` are `null` before a phase begins.
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

/// The title for an issue block: the folded `IssueEntry.title` when non-empty, else
/// the `RunState.queue` seed (populated by `queue.built`), else an empty string.
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

/// The reserved `data.issue` block (ADR-0019 amendment #96) carried on every
/// subject-scoped event: `{number, title}` with the title resolved via
/// [`issue_title`].
fn issue_block(state: &RunState, number: u64) -> Value {
    json!({ "number": number, "title": issue_title(state, number) })
}

/// Assemble a CloudEvents 1.0 structured-mode envelope. `data` is the
/// event-specific object; the reserved `emitter`/`git` blocks are merged into it,
/// the contextual `agent` block on every type except `run.finished`, and the `issue`
/// block on every subject-scoped event — the envelope still carries exactly one
/// extension attribute, `runid` (ADR-0019 §3).
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
        // The contextual agent block rides every event except `run.finished`.
        if type_ != "dev.ralphy.run.finished" {
            map.insert("agent".to_string(), agent_block(state));
        }
        // The issue block rides exactly the subject-scoped events (`issue.*`,
        // `plan.*`); run-scoped events (no subject) carry none.
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

/// Build a subject-less envelope for a synthetic `run.*` event that has no
/// [`RunEvent`] source (the sink's own `run.heartbeat`). Shares the exact envelope
/// assembly — specversion, id, time, runid, merged emitter — as the mapped events.
pub fn run_envelope(type_: &str, ctx: &EventCtx, state: &RunState, data: Value) -> Value {
    envelope(type_, None, ctx, state, data)
}

/// Build a `dev.ralphy.plan.step` envelope (#96): a single checkbox transition to
/// `checked`/`noticed`, subject-scoped to `issue/<number>` (so the reserved `issue`
/// block rides along), carrying `data = {text, status}` (the normalized step text).
/// Emitted by the sink's plan-step poller, which owns the file-state diff.
pub fn plan_step_envelope(
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

/// The `data` payload shared by the enriched `queue.built` and the on-demand
/// `queue.snapshot` (ADR-0020): `{count, order, stop_before, issues,
/// assignee_filter}`. Defined ONCE and used by both triggers so the two payloads
/// can never diverge — `issues` is the per-issue snapshot array (`Value::Null` for
/// the legacy shape when the resolver produced none). `assignee_filter` is the
/// resolved concrete login the queue was scoped to (ADR-0021 §5), `null` = whole
/// queue. The reserved `emitter` block is merged in later by
/// [`envelope`]/[`run_envelope`].
pub fn queue_snapshot_data(
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

/// Wrap a [`queue_snapshot_data`] payload in the `dev.ralphy.queue.snapshot`
/// envelope (ADR-0020): the on-demand `ralphy issues --push` emission, byte-for-byte
/// the same `data` shape the runner's `queue.built` carries, only the envelope
/// `type` differs. Subject-less, like `queue.built`. The `state` is the caller's
/// (out-of-run `ralphy issues --push` passes a default, so the `agent` block is
/// all-`null` — matching `queue.built` before `run.started` folds).
pub fn queue_snapshot_envelope(data: Value, ctx: &EventCtx, state: &RunState) -> Value {
    run_envelope("dev.ralphy.queue.snapshot", ctx, state, data)
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
            assignee_filter,
            // LOG-ONLY (#222): the console notice's scope phrase never reaches the
            // wire — `dev.ralphy.queue.built`'s shape is unchanged.
            scope: _,
        } => Some(envelope(
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
        )),
        RunEvent::IssueStarted { number, title } => Some(envelope(
            "dev.ralphy.issue.started",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number, "title": title }),
        )),
        RunEvent::Planning { model, effort } => {
            // The adapter event carries no number; the subject is the active issue.
            let subject = state.active.map(subject_for);
            Some(envelope(
                "dev.ralphy.issue.planning",
                subject.as_deref(),
                ctx,
                state,
                json!({ "model": model, "effort": effort }),
            ))
        }
        RunEvent::PlanWritten {
            number,
            open_steps,
            usage,
            steps,
        } => {
            // The parsed checkbox steps as `[{text,status}]` (#96) — sourced from the
            // runner's `steps_json`, so the mapper stays pure (no file I/O).
            let steps: Vec<Value> = steps
                .iter()
                .map(|(text, status)| json!({ "text": text, "status": status }))
                .collect();
            Some(envelope(
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
            ))
        }
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
                state,
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
            state,
            json!({ "number": number, "tokens": tokens, "usage": usage_json(usage) }),
        )),
        RunEvent::NonGreen { number, outcome } => Some(envelope(
            "dev.ralphy.issue.non_green",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number, "outcome": outcome }),
        )),
        RunEvent::NeedsSplit { number } => Some(envelope(
            "dev.ralphy.issue.needs_split",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number }),
        )),
        RunEvent::Skipped {
            number,
            kind,
            label,
            blockers,
        } => Some(envelope(
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
        )),
        RunEvent::HumanBlocked { number, on } => Some(envelope(
            "dev.ralphy.issue.human_blocked",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number, "on": on }),
        )),
        // The run declined to start (#222): run-scoped, so no `subject`.
        RunEvent::RunSkipped { reason } => Some(envelope(
            "dev.ralphy.run.skipped",
            None,
            ctx,
            state,
            json!({ "reason": reason }),
        )),
        RunEvent::DeadlinePassed { number } => Some(envelope(
            "dev.ralphy.issue.deadline_passed",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number }),
        )),
        RunEvent::SleepStarted {
            reset,
            target_epoch,
        } => Some(envelope(
            "dev.ralphy.run.sleep_started",
            None,
            ctx,
            state,
            json!({ "reset": reset, "target_epoch": target_epoch }),
        )),
        RunEvent::SleepEnded => Some(envelope(
            "dev.ralphy.run.sleep_ended",
            None,
            ctx,
            state,
            json!({}),
        )),
        RunEvent::ApiDegraded => Some(envelope(
            "dev.ralphy.run.api_degraded",
            None,
            ctx,
            state,
            json!({}),
        )),
        RunEvent::ApiRecovered => Some(envelope(
            "dev.ralphy.run.api_recovered",
            None,
            ctx,
            state,
            json!({}),
        )),
        // The reap carries its window so a consumer can tell a tight operator
        // setting from the path default without reading the run's config.
        RunEvent::IdleReaped { idle_minutes } => Some(envelope(
            "dev.ralphy.run.idle_reaped",
            None,
            ctx,
            state,
            json!({ "idle_minutes": idle_minutes }),
        )),
        RunEvent::KnowledgeConsolidating { notes } => Some(envelope(
            "dev.ralphy.knowledge.consolidating",
            None,
            ctx,
            state,
            json!({ "notes": notes }),
        )),
        RunEvent::KnowledgeConsolidated { archived } => Some(envelope(
            "dev.ralphy.knowledge.consolidated",
            None,
            ctx,
            state,
            json!({ "archived": archived }),
        )),
        RunEvent::Notice { level, message } => Some(envelope(
            "dev.ralphy.run.notice",
            None,
            ctx,
            state,
            json!({ "level": level.to_string().to_lowercase(), "message": message }),
        )),
        RunEvent::RunStarted {
            repo,
            queue_labels,
            // The exec agent is now conveyed uniformly by the `data.agent` block
            // (`data.agent.name`), so the redundant scalar is dropped here; the
            // distinct plan agent stays a scalar (the block only carries one name).
            agent: _,
            plan_agent,
            branch_mode,
            branch,
            deadline_hours,
        } => {
            // The light scope list seeded by the preceding `queue.built` (#96) —
            // `[{number,title}]`; the rich ADR-0020 snapshot stays on `queue.built`.
            let queue: Vec<Value> = state
                .queue
                .iter()
                .map(|q| json!({ "number": q.number, "title": q.title }))
                .collect();
            Some(envelope(
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
            ))
        }
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
        } => {
            // The per-issue rollup (#96): every issue that entered the fold and
            // reached a terminal status, with its granular `status` and (only on a
            // skip) the skip `kind`. The scalar counts stay for completeness.
            let issues: Vec<Value> = state
                .issues
                .iter()
                .filter_map(|e| {
                    let status = e.status.status_wire()?;
                    let mut obj = serde_json::Map::new();
                    obj.insert("number".to_string(), json!(e.number));
                    obj.insert("title".to_string(), json!(issue_title(state, e.number)));
                    obj.insert("status".to_string(), json!(status));
                    if status == "skipped" {
                        if let Some(kind) = e.kind {
                            obj.insert("kind".to_string(), json!(skip_kind_name(kind)));
                        }
                        obj.insert("blocked_by".to_string(), json!(e.blocked_by));
                    }
                    Some(Value::Object(obj))
                })
                .collect();
            Some(envelope(
                "dev.ralphy.run.finished",
                None,
                ctx,
                state,
                json!({
                    "outcome": outcome,
                    "issues_done": issues_done,
                    "issues_skipped": issues_skipped,
                    "issues_total": issues_total,
                    "issues": issues,
                    "tokens_total": { "up": up, "cr": cr, "cw": cw, "out": out },
                    "duration_s": duration_s,
                }),
            ))
        }
        // The raw plan snapshots (#96): issue-scoped, carrying the complete `plan.md`
        // under `data.plan_md`. The reserved `issue` block rides via the subject.
        RunEvent::PlanOpened { number, plan_md } => Some(envelope(
            "dev.ralphy.plan.opened",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number, "plan_md": plan_md }),
        )),
        RunEvent::PlanClosed { number, plan_md } => Some(envelope(
            "dev.ralphy.plan.closed",
            Some(&subject_for(*number)),
            ctx,
            state,
            json!({ "number": number, "plan_md": plan_md }),
        )),
    }
}

#[cfg(test)]
mod tests;
