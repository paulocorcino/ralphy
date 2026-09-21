//! Rendering, pure over `&RunState`: the card, its footer, the sleep line and
//! the disposable pushes (ADR-0007 D3, D6).

use crate::runstate::{IssueEntry, IssueStatus, RunState, SkipKind, SleepState};

/// Telegram's hard per-message character limit.
const TELEGRAM_LIMIT: usize = 4096;

/// Above this many issues the card collapses to counters + active + last-finished
/// rather than one line per issue (ADR-0007 D6).
const FULL_LIST_MAX: usize = 30;

/// The status emoji for an issue (ADR-0007 D3 icon table).
fn status_emoji(status: &IssueStatus) -> &'static str {
    match status {
        IssueStatus::Planning => "🧠",
        IssueStatus::Executing => "⚙️",
        IssueStatus::Planned => "📝",
        IssueStatus::Done => "✅",
        IssueStatus::Skipped => "⏭️",
        IssueStatus::Blocked => "⛔",
        IssueStatus::Infeasible => "🤷",
        IssueStatus::NeedsSplit => "🧩",
        IssueStatus::NonGreen => "❌",
        IssueStatus::Hitl => "🙋",
    }
}

/// One rendered issue line: `emoji #n title`. The card carries no per-issue clock —
/// the budget is a static ceiling (e.g. `90:00`), not elapsed time, so showing it as
/// a clock only misleads. A dependency skip appends ` (blocked by #N)` when the
/// gating blocker(s) are known, so the operator sees which issue held it.
fn issue_line(entry: &IssueEntry) -> String {
    let emoji = status_emoji(&entry.status);
    let blocked_by = if entry.status == IssueStatus::Skipped
        && entry.kind == Some(SkipKind::BlockedBy)
        && !entry.blocked_by.is_empty()
    {
        let by = entry
            .blocked_by
            .iter()
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" (blocked by {by})")
    } else {
        String::new()
    };
    format!("{emoji} #{} {}{blocked_by}", entry.number, entry.title)
}

/// The card's counter line, e.g. `▶️ 4 · ✅ 2 · ⏭️ 1 · ⛔ 0 · 🤷 0 · ❌ 0`. The
/// leading `▶️ N` is the queue total (ADR-0007 D3 consolidated card). A `🧩 N`
/// needs-split counter appears only when non-zero — the common card stays
/// unchanged, but a parked-on-split run is visibly different.
fn counters_line(state: &RunState) -> String {
    let c = state.counts();
    let mut line = format!(
        "▶️ {} · ✅ {} · ⏭️ {} · ⛔ {} · 🤷 {} · ❌ {}",
        state.total, c.done, c.skipped, c.blocked, c.infeasible, c.non_green
    );
    if c.needs_split > 0 {
        line.push_str(&format!(" · 🧩 {}", c.needs_split));
    }
    // A `📝 N` counter appears only on a plan-only (dry-run) pass, so the common
    // card stays unchanged but a dry run is visibly not a stalled one.
    if c.planned > 0 {
        line.push_str(&format!(" · 📝 {}", c.planned));
    }
    // A `🙋 N` counter appears only when a chain is parked on a human gate
    // (ADR-0014) — the common card stays unchanged, but a run waiting on a
    // person is visibly different.
    if c.hitl > 0 {
        line.push_str(&format!(" · 🙋 {}", c.hitl));
    }
    line
}

/// The card's branding header: `🦊 Ralphy - v0.1.0` — a stable per-run face (seeded
/// by the run title) plus the binary's own version. Shared with the console header.
fn header_line(state: &RunState) -> String {
    crate::runstate::ralphy_header(&state.title)
}

/// The issue list block: one line per issue for a small queue, or the collapsed
/// `active`/`last` pair above [`FULL_LIST_MAX`] (ADR-0007 D6). Lines are joined by a
/// single `\n`; the caller separates this block from its neighbours with a blank
/// line. Empty when no issue has entered the lifecycle yet.
fn render_issue_block(state: &RunState) -> String {
    let mut lines: Vec<String> = Vec::new();
    if state.issues.len() <= FULL_LIST_MAX {
        for entry in &state.issues {
            lines.push(issue_line(entry));
        }
    } else {
        if let Some(active) = state.active_issue() {
            lines.push(format!("active · {}", issue_line(active)));
        }
        if let Some(last) = state.most_recent_finished() {
            lines.push(format!("last  · {}", issue_line(last)));
        }
    }
    lines.join("\n")
}

/// The live sleep line (ADR-0007 D3): `🌙 waiting for reset ~HH:MM · resumes in
/// ~Xh Ym`. The `HH:MM` is the event's raw reset hint; the countdown is
/// `max(0, target_epoch - now_epoch)` so it degrades to `~0m` once the reset is
/// due rather than going negative.
fn render_sleep_line(sleep: &SleepState, now_epoch: i64) -> String {
    let remaining = (sleep.target_epoch - now_epoch).max(0);
    let total_min = remaining / 60;
    let (h, m) = (total_min / 60, total_min % 60);
    let countdown = if h > 0 {
        format!("~{h}h {m}m")
    } else {
        format!("~{m}m")
    };
    format!(
        "🌙 waiting for reset ~{} · resumes in {}",
        sleep.reset, countdown
    )
}

/// Render the live card from a [`RunState`], guaranteed within Telegram's
/// 4096-char limit. A small queue renders one line per issue; a large one (over
/// [`FULL_LIST_MAX`]) collapses to the counters plus the active issue and the
/// most-recently-finished one (ADR-0007 D6). `now_epoch` (Unix seconds) anchors
/// the live sleep countdown.
pub fn render_card(state: &RunState, now_epoch: i64) -> String {
    // The card is one consolidated component: four groups separated by a blank
    // line (ADR-0007 D3). Each group is a section string; only non-empty sections
    // join, so a not-yet-started run (no issues) never leaves a stray blank line.
    let mut sections: Vec<String> = Vec::new();

    // 1) Branding header: a stable per-run face + the binary version.
    sections.push(header_line(state));

    // 2) Title + counters — one group, no blank line between the two lines.
    sections.push(format!("{}\n{}", state.title, counters_line(state)));

    // 3) The live sleep block, its own group while waiting for a reset.
    if let Some(sleep) = &state.sleep {
        sections.push(render_sleep_line(sleep, now_epoch));
    }

    // 4) The issue list (collapsed above FULL_LIST_MAX).
    let issues = render_issue_block(state);
    if !issues.is_empty() {
        sections.push(issues);
    }

    // 4b) The live knowledge-consolidation line (end-of-run trigger), its own group
    // while the session runs. Hidden once `finished` so a failed session — which
    // never clears `consolidating` — leaves no stale line on the terminal card; a
    // successful one is summarised in the footer instead.
    if let Some(notes) = state.consolidating {
        if !state.finished {
            sections.push(format!(
                "📚 consolidating {notes} knowledge note(s) into KNOWLEDGE.md…"
            ));
        }
    }

    // 5) The terminal footer — only once the run has finished, so the issue list
    // is the last group through the live run.
    if state.finished {
        sections.push(render_final_push(state));
    }

    truncate_chars(sections.join("\n\n"), TELEGRAM_LIMIT)
}

/// The run's terminal footer, embedded as the last group of the consolidated card
/// (`🏁 <title> — <head> · ✅ N done, ⏭️ M skipped`). Bounded to the message limit so
/// an over-long `--title` cannot make Telegram reject the edit.
pub(super) fn render_final_push(state: &RunState) -> String {
    let c = state.counts();
    // A run that reaches its terminal edge without a single issue finishing,
    // skipping, or parking never actually did any work — it was interrupted
    // (killed, superseded, or bailed at startup before the first `IssueStarted`).
    // The celebratory `🏁 … ✅ 0 done` footer misreads that as a clean completion:
    // an aborted run's card then sits above the next run's fresh card and reads
    // "finished → started" (FinCal, 2026-07-13). Render a distinct stopped footer so
    // a no-op run never masquerades as a completed one.
    let processed = c.done
        + c.skipped
        + c.blocked
        + c.infeasible
        + c.non_green
        + c.needs_split
        + c.hitl
        + c.planned;
    if processed == 0 {
        // A run border that folded its OWN summary (a `--if-idle` deferral, #222)
        // knows why it processed nothing — say that instead of the generic stop,
        // which would read as an unexplained abort.
        let head = state
            .final_summary
            .clone()
            .unwrap_or_else(|| "stopped before any issue was processed".to_string());
        return truncate_chars(format!("🛑 {} — {head}", state.title), TELEGRAM_LIMIT);
    }
    let head = state
        .final_summary
        .clone()
        .unwrap_or_else(|| "run finished".to_string());
    // A bundle verdict parks the queue on a human split — the footer must say
    // so, or a run that ends "green" hides the pending human step.
    let split_part = if c.needs_split > 0 {
        format!(", 🧩 {} awaiting split", c.needs_split)
    } else {
        String::new()
    };
    // A chain parked on a human gate (ADR-0014) is a pending human step the
    // footer must name, or a run that ends "green" hides it.
    let hitl_part = if c.hitl > 0 {
        format!(", 🙋 {} waiting on human", c.hitl)
    } else {
        String::new()
    };
    // The end-of-run knowledge consolidation, when it ran: a `📚 N consolidated`
    // segment so the curation step is visible on the terminal card.
    let knowledge_part = match state.consolidated {
        Some(n) => format!(", 📚 {n} consolidated"),
        None => String::new(),
    };
    truncate_chars(
        format!(
            "🏁 {} — {} · ✅ {} done, ⏭️ {} skipped{hitl_part}{split_part}{knowledge_part}",
            state.title, head, c.done, c.skipped
        ),
        TELEGRAM_LIMIT,
    )
}

/// The push sent on entering a usage-limit sleep (a new message so the phone
/// buzzes, ADR-0007 D3). Bounded like the other pushes.
pub fn render_sleep_push(state: &RunState) -> String {
    let reset = state.sleep.as_ref().map(|s| s.reset.as_str()).unwrap_or("");
    truncate_chars(
        format!(
            "🌙 {} — usage limit, waiting for reset ~{}",
            state.title, reset
        ),
        TELEGRAM_LIMIT,
    )
}

/// The push sent when the active child enters a sustained API-degraded state
/// (issue #149). A new message so the phone buzzes, mirroring the sleep push.
pub fn render_degraded_push(state: &RunState) -> String {
    truncate_chars(
        format!("⚠️ {} — API degraded, child retrying", state.title),
        TELEGRAM_LIMIT,
    )
}

/// The push sent when the idle watchdog reaps the child. Closes a pending
/// degraded push *without* claiming recovery — the episode ended because Ralphy
/// killed the child, which is the opposite news.
pub fn render_idle_reaped_push(state: &RunState, idle_minutes: u64) -> String {
    truncate_chars(
        format!(
            "💤 {} — no progress for {idle_minutes} min, child reaped",
            state.title
        ),
        TELEGRAM_LIMIT,
    )
}

/// The push sent when the agent asks for the operator (ADR-0059 §2): a
/// `waiting` state is a buzz, with what it is asking when the hook said.
pub fn render_waiting_push(state: &RunState, detail: Option<&str>) -> String {
    truncate_chars(
        format!(
            "🙋 {} — agent is waiting: {}",
            state.title,
            detail.unwrap_or("input needed")
        ),
        TELEGRAM_LIMIT,
    )
}

/// The push sent when the API recovers, matching a prior degraded push.
pub fn render_recover_push(state: &RunState) -> String {
    truncate_chars(
        format!("✅ {} — API recovered, resuming", state.title),
        TELEGRAM_LIMIT,
    )
}

/// Truncate `s` to at most `max` characters on a char boundary.
fn truncate_chars(mut s: String, max: usize) -> String {
    if s.chars().count() <= max {
        return s;
    }
    let idx = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    s.truncate(idx);
    s
}

/// Derive the card title (ADR-0007 D3): `--title` wins; else the single issue's
/// title with `--only-issue`; else `<repo> · N issues [labels]`.
pub fn derive_title(
    repo_name: &str,
    issue_count: usize,
    labels: &[String],
    only_issue_title: Option<&str>,
    title_override: Option<&str>,
) -> String {
    if let Some(t) = title_override {
        if !t.trim().is_empty() {
            return t.to_string();
        }
    }
    if let Some(t) = only_issue_title {
        return t.to_string();
    }
    let label_part = if labels.is_empty() {
        String::new()
    } else {
        format!(" [{}]", labels.join(", "))
    };
    format!("{repo_name} · {issue_count} issues{label_part}")
}

#[cfg(test)]
mod tests;
