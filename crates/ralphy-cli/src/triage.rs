//! `ralphy triage` (ADR-0017): the agent-triage entry path. For each open issue
//! carrying `triage-agent`, an agent session judges promote / consolidate /
//! bounce, and the cli applies the verdicts — a local preview the operator
//! confirms, or `--yes` for schedulers. The trust act already happened when the
//! operator applied `triage-agent`, so `--yes` promotion is a mechanical
//! continuation of a human decision, not the agent expanding its own authority.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Args;
use ralphy_core::{
    git, github, GhTracker, IssueTracker, TriageDraft, TriageItem, TriageRequest, TriageVerdict,
    Workspace, CONSOLIDATED_SPEC_MARKER, PROMOTE_EVIDENCE_MARKER, TRIAGE_AGENT_LABEL,
};

use crate::init::{
    agent_logged_in, agent_present, resolve_human_label, resolve_triage_label, Agent,
};
use crate::runlock::{self, LockState};

/// The canonical reporter-bounce label a `bounce` verdict swaps in.
const NEEDS_INFO_LABEL: &str = "needs-info";

#[derive(Args)]
pub struct TriageArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    repo: std::path::PathBuf,

    /// Which agent CLI drives the triage judgment. Must be logged in. Defaults to
    /// the first logged-in agent (claude, then codex, then opencode).
    #[arg(long, value_enum)]
    agent: Option<Agent>,

    /// Model for the triage session (agent default when omitted).
    #[arg(long)]
    model: Option<String>,

    /// Reasoning effort for the triage session.
    #[arg(long, default_value = "medium")]
    effort: String,

    /// Wall-clock budget (minutes) before the session is reclaimed.
    #[arg(long, default_value_t = 20)]
    max_minutes: u64,

    /// Publish and promote directly without the interactive confirm (for
    /// schedulers). The trust act already happened at labelling time.
    #[arg(long)]
    yes: bool,

    /// Skip this invocation (exit 0) when another Ralphy process (run or
    /// triage) is already active in the repo — the anti-overlap flag scheduled
    /// invocations pass. Without it a live lock only warns.
    #[arg(long)]
    if_idle: bool,
}

/// What the repo-scoped presence lock (`.ralphy/run.lock`) means for this
/// invocation of `triage`, given `--if-idle` and the lock's inspected state.
#[derive(Debug)]
enum PresenceGate {
    /// Proceed, optionally warning first.
    Proceed { warn: Option<String> },
    /// Exit 0 with this message instead of proceeding.
    Defer(String),
}

/// Format an RFC 3339 timestamp as `%Y-%m-%d %H:%M:%S`, falling back to the raw
/// string on parse failure (mirrors `run_cmd`'s formatting, run.rs L76-78).
fn format_since(started_at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(started_at)
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| started_at.to_string())
}

/// Classify the repo-scoped presence lock into what `triage::run` should do.
/// Pure over an injected liveness predicate so it unit-tests without a real
/// second process (mirrors `runlock::inspect`'s own test pattern).
fn presence_gate(path: &Path, if_idle: bool, is_alive: impl Fn(u32) -> bool) -> PresenceGate {
    match runlock::inspect(path, is_alive) {
        LockState::HeldAlive(info) if if_idle => PresenceGate::Defer(format!(
            "skipped: run in progress since {}, pid {}",
            format_since(&info.started_at),
            info.pid
        )),
        LockState::HeldAlive(info) => PresenceGate::Proceed {
            warn: Some(format!(
                "a run is already active in this repo — proceeding anyway (pid {})",
                info.pid
            )),
        },
        LockState::Stale(info) => PresenceGate::Proceed {
            warn: Some(format!(
                "ignoring stale run.lock (pid {} not running)",
                info.pid
            )),
        },
        LockState::Corrupt => PresenceGate::Proceed {
            warn: Some("ignoring unreadable run.lock".into()),
        },
        LockState::Free => PresenceGate::Proceed { warn: None },
    }
}

/// The resolved label strings a triage apply swaps between, named once so the
/// pure [`apply_triage`] core stays free of `gh`/repo lookups.
pub struct TriageLabels {
    /// The label `triage-agent` is removed in favour of on promote/consolidate.
    pub queue_label: String,
    /// The label `triage-agent` is removed in favour of on bounce.
    pub needs_info_label: String,
    /// The human-return label `triage-agent` is removed in favour of on escalate
    /// (mapping-resolved `ready-for-human`, ADR-0018 §3).
    pub human_label: String,
    /// The operational label a triaged issue carries coming in.
    pub triage_agent_label: String,
    /// The consolidated-spec comment marker.
    pub marker: String,
    /// The promote evidence-stamp comment marker (ADR-0027).
    pub promote_marker: String,
}

/// Dispatch the triage session to the selected agent's adapter. The charter is
/// shared ([`ralphy_core::build_triage_prompt`]); only the invocation differs.
fn triage_with_agent(
    agent: Agent,
    repo: &Path,
    out_path: &Path,
    req: &TriageRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<TriageDraft> {
    match agent {
        Agent::Claude => {
            ralphy_agent_claude::triage_issues(repo, out_path, req, model, effort, timeout)
        }
        Agent::Codex => {
            ralphy_agent_codex::triage_issues(repo, out_path, req, model, effort, timeout)
        }
        Agent::Copilot => {
            ralphy_agent_copilot::triage_issues(repo, out_path, req, model, effort, timeout)
        }
        Agent::Gemini => {
            ralphy_agent_gemini::triage_issues(repo, out_path, req, model, effort, timeout)
        }
        Agent::Cursor => {
            ralphy_agent_cursor::triage_issues(repo, out_path, req, model, effort, timeout)
        }
        Agent::Kimi => {
            ralphy_agent_kimi::triage_issues(repo, out_path, req, model, effort, timeout)
        }
        Agent::Opencode => {
            ralphy_agent_opencode::triage_issues(repo, out_path, req, model, effort, timeout)
        }
    }
}

/// Choose the triage agent: an explicit `--agent` must be logged in; otherwise the
/// first logged-in agent in gate order. Errors when none is logged in.
fn select_triage_agent(requested: Option<Agent>) -> Result<Agent> {
    // `agent_present` first, as `ralphy init` does: a login probe spawns the
    // vendor CLI (Copilot's is a real subprocess + CAPI fetch), so probing an
    // absent binary is pure wall-clock.
    let logged_in: Vec<Agent> = Agent::ALL
        .into_iter()
        .filter(agent_present)
        .filter(agent_logged_in)
        .collect();
    match requested {
        Some(a) if logged_in.contains(&a) => Ok(a),
        Some(a) => bail!(
            "ralphy triage: --agent {} is not logged in (logged in: {})",
            a.cli_name(),
            if logged_in.is_empty() {
                "none".to_string()
            } else {
                logged_in
                    .iter()
                    .map(|x| x.cli_name())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
        None => logged_in.first().copied().context(
            "no logged-in agent CLI found — log in to claude, codex, or opencode and retry",
        ),
    }
}

/// A one-line preview of a verdict for the confirm prompt.
fn verdict_line(item: &TriageItem, labels: &TriageLabels) -> String {
    match item.verdict {
        TriageVerdict::Promote => format!(
            "  #{}: promote — post evidence stamp, swap {} → {}",
            item.number, labels.triage_agent_label, labels.queue_label
        ),
        TriageVerdict::Consolidate => format!(
            "  #{}: consolidate — post spec comment, swap {} → {}",
            item.number, labels.triage_agent_label, labels.queue_label
        ),
        TriageVerdict::Bounce => format!(
            "  #{}: bounce — comment, swap {} → {}",
            item.number, labels.triage_agent_label, labels.needs_info_label
        ),
        TriageVerdict::Escalate => format!(
            "  #{}: escalate — comment, swap {} → {}",
            item.number, labels.triage_agent_label, labels.human_label
        ),
    }
}

/// Apply the triage verdicts through the tracker. `decide` gates the outward
/// promote/consolidate publishes (the operator's confirm; `--yes` passes
/// `|_| true`); the bounce arm never consults it — returning work to a human is
/// always safe (ADR-0017 §5). Label swaps use `remove_label` + `add_label`;
/// promote (its evidence stamp, ADR-0027) and consolidate (its spec) both
/// post-or-edit their marked comment (idempotent). Pure over the
/// [`IssueTracker`] trait so it unit-tests against a recording fake.
pub fn apply_triage(
    draft: &TriageDraft,
    tracker: &dyn IssueTracker,
    labels: &TriageLabels,
    decide: impl Fn(&TriageItem) -> bool,
) -> Result<()> {
    for item in &draft.items {
        match item.verdict {
            TriageVerdict::Promote => {
                if !decide(item) {
                    continue;
                }
                // ADR-0027: promote records its evidence stamp before admitting
                // the issue to the queue. Upsert (not plain comment) so re-triage
                // edits the marked comment rather than stacking a second one.
                let body = item.comment.as_deref().unwrap_or_default();
                tracker.upsert_marked_comment(item.number, &labels.promote_marker, body)?;
                tracker.remove_label(item.number, &labels.triage_agent_label)?;
                tracker.add_label(item.number, &labels.queue_label)?;
            }
            TriageVerdict::Consolidate => {
                if !decide(item) {
                    continue;
                }
                let body = item.comment.as_deref().unwrap_or_default();
                tracker.upsert_marked_comment(item.number, &labels.marker, body)?;
                tracker.remove_label(item.number, &labels.triage_agent_label)?;
                tracker.add_label(item.number, &labels.queue_label)?;
            }
            TriageVerdict::Bounce => {
                // Returning work to a human is always safe: never gated on `decide`.
                if let Some(body) = item.comment.as_deref() {
                    tracker.comment(item.number, body)?;
                }
                tracker.remove_label(item.number, &labels.triage_agent_label)?;
                tracker.add_label(item.number, &labels.needs_info_label)?;
            }
            TriageVerdict::Escalate => {
                // Handing a decision to a maintainer is always safe: never gated
                // on `decide` (ADR-0018 §3). Comment + swap only — any proposed
                // follow-up issue is created interactively in `run()`, never here,
                // so the `--yes` path can never author a new issue.
                if let Some(body) = item.comment.as_deref() {
                    tracker.comment(item.number, body)?;
                }
                tracker.remove_label(item.number, &labels.triage_agent_label)?;
                tracker.add_label(item.number, &labels.human_label)?;
            }
        }
    }
    Ok(())
}

/// `ralphy triage`: list the `triage-agent` issues, run the judgment session,
/// preview the verdicts, and apply them on confirm (or `--yes`).
pub fn run(args: &TriageArgs) -> Result<()> {
    let repo = git::resolve_toplevel(&args.repo)?;
    crate::daemon::register_repo(&repo);

    let ws = Workspace::new(&repo);
    std::fs::create_dir_all(ws.ralphy_dir()).ok();
    let lock_path = ws.run_lock_path();
    match presence_gate(&lock_path, args.if_idle, runlock::pid_is_alive) {
        PresenceGate::Defer(msg) => {
            println!("{msg}");
            return Ok(());
        }
        PresenceGate::Proceed { warn } => {
            if let Some(w) = warn {
                eprintln!("{w}");
            }
        }
    }
    let _lock = runlock::acquire(&lock_path)?;

    // The labelled subset — tens, not hundreds. `triage-agent` is a fixed
    // operational label, never remapped.
    // Triage sweeps the whole repo's queue label — never assignee-scoped (ADR-0021).
    let issues = github::list_queue(&[TRIAGE_AGENT_LABEL.to_string()], None, &repo)?;
    if issues.is_empty() {
        println!("No open issue carries `triage-agent` — nothing to triage.");
        return Ok(());
    }
    let numbers: Vec<u64> = issues.iter().map(|i| i.number).collect();
    println!(
        "Triaging {} issue(s): {}",
        numbers.len(),
        numbers
            .iter()
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let agent = select_triage_agent(args.agent)?;
    let queue_label = resolve_triage_label(&repo);
    let labels = TriageLabels {
        queue_label: queue_label.clone(),
        needs_info_label: NEEDS_INFO_LABEL.to_string(),
        human_label: resolve_human_label(&repo),
        triage_agent_label: TRIAGE_AGENT_LABEL.to_string(),
        marker: CONSOLIDATED_SPEC_MARKER.to_string(),
        promote_marker: PROMOTE_EVIDENCE_MARKER.to_string(),
    };

    // Mechanically pre-fetch text + (capability-gated) image attachments as
    // guardrailed evidence (ADR-0025). Best-effort: only a temp-dir creation
    // failure errors; downloads never abort.
    let images = github::ImageCapability {
        accepts: agent.accepts_images(),
        adapter_name: agent.cli_name(),
    };
    let attachments = github::fetch_triage_attachments(&repo, &numbers, images)?;

    let out_path = repo.join(".ralphy").join("triage-draft.json");
    let req = TriageRequest {
        issue_numbers: &numbers,
        queue_label: &queue_label,
        attachments_manifest: &attachments.manifest,
        image_paths: &attachments.image_paths,
    };
    let draft = triage_with_agent(
        agent,
        &repo,
        &out_path,
        &req,
        args.model.as_deref(),
        Some(&args.effort),
        Duration::from_secs(args.max_minutes * 60),
    )?;
    // Pin the TempDir alive until AFTER the session has read the files; its Drop
    // deletes the attachment dir last (ADR-0025 §7).
    drop(attachments);
    draft
        .validate()
        .map_err(|reason| anyhow::anyhow!("triage draft is invalid: {reason}"))?;

    // Preview every outward action before publishing (ADR-0012 posture).
    println!("\nTriage verdicts:");
    for item in &draft.items {
        println!("{}", verdict_line(item, &labels));
    }

    let tracker = GhTracker::new(&repo);
    if args.yes {
        apply_triage(&draft, &tracker, &labels, |_| true)?;
        println!("\nApplied {} verdict(s).", draft.item_count());
        return Ok(());
    }

    // Interactive confirm: default No, so a bulk external write is never silent.
    // Bounces apply regardless (safe); the confirm gates promote/consolidate.
    print!("\n  > Publish these verdicts? [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading answer from stdin")?;
    let confirmed = matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    apply_triage(&draft, &tracker, &labels, |_| confirmed)?;
    if confirmed {
        println!("Applied {} verdict(s).", draft.item_count());
    } else {
        println!("Declined — promote/consolidate skipped; any bounces/escalates were applied.");
    }

    // Interactive redirect flow (ADR-0018 §4): a drafted follow-up issue is
    // created ONLY after an explicit per-draft `y`. Never reached under `--yes`
    // (that branch returns above), so escalate can never author a new issue
    // non-interactively.
    for item in &draft.items {
        if item.verdict != TriageVerdict::Escalate {
            continue;
        }
        let Some(draft_issue) = &item.draft_issue else {
            continue;
        };
        println!(
            "\nEscalate #{} proposes a follow-up issue:\n  title: {}\n  body:\n{}",
            item.number,
            draft_issue.title,
            indent_body(&draft_issue.body)
        );
        print!("  > Create this follow-up issue? [y/N] ");
        std::io::stdout().flush().ok();
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .context("reading answer from stdin")?;
        if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            let number =
                tracker.create_issue(&draft_issue.title, &draft_issue.body, &draft_issue.labels)?;
            println!("  created follow-up issue #{number}.");
        } else {
            println!("  skipped — no issue created.");
        }
    }
    Ok(())
}

/// Indent a multi-line issue body for the interactive preview.
fn indent_body(body: &str) -> String {
    body.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
