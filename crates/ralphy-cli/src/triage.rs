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
    CONSOLIDATED_SPEC_MARKER, TRIAGE_AGENT_LABEL,
};

use crate::init::{agent_logged_in, resolve_triage_label, Agent};

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
}

/// The resolved label strings a triage apply swaps between, named once so the
/// pure [`apply_triage`] core stays free of `gh`/repo lookups.
pub struct TriageLabels {
    /// The label `triage-agent` is removed in favour of on promote/consolidate.
    pub queue_label: String,
    /// The label `triage-agent` is removed in favour of on bounce.
    pub needs_info_label: String,
    /// The operational label a triaged issue carries coming in.
    pub triage_agent_label: String,
    /// The consolidated-spec comment marker.
    pub marker: String,
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
        Agent::Opencode => {
            ralphy_agent_opencode::triage_issues(repo, out_path, req, model, effort, timeout)
        }
    }
}

/// Choose the triage agent: an explicit `--agent` must be logged in; otherwise the
/// first logged-in agent in gate order. Errors when none is logged in.
fn select_triage_agent(requested: Option<Agent>) -> Result<Agent> {
    let logged_in: Vec<Agent> = Agent::ALL.into_iter().filter(agent_logged_in).collect();
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
            "  #{}: promote — swap {} → {}",
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
    }
}

/// Apply the triage verdicts through the tracker. `decide` gates the outward
/// promote/consolidate publishes (the operator's confirm; `--yes` passes
/// `|_| true`); the bounce arm never consults it — returning work to a human is
/// always safe (ADR-0017 §5). Label swaps use `remove_label` + `add_label`; a
/// consolidate posts-or-edits the marked comment (idempotent). Pure over the
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
        }
    }
    Ok(())
}

/// `ralphy triage`: list the `triage-agent` issues, run the judgment session,
/// preview the verdicts, and apply them on confirm (or `--yes`).
pub fn run(args: &TriageArgs) -> Result<()> {
    let repo = git::resolve_toplevel(&args.repo)?;

    // The labelled subset — tens, not hundreds. `triage-agent` is a fixed
    // operational label, never remapped.
    let issues = github::list_queue(&[TRIAGE_AGENT_LABEL.to_string()], &repo)?;
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
        triage_agent_label: TRIAGE_AGENT_LABEL.to_string(),
        marker: CONSOLIDATED_SPEC_MARKER.to_string(),
    };

    let out_path = repo.join(".ralphy").join("triage-draft.json");
    let req = TriageRequest {
        issue_numbers: &numbers,
        queue_label: &queue_label,
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
        println!("Declined — promote/consolidate skipped; any bounces were applied.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct RecordingTracker {
        added: RefCell<Vec<(u64, String)>>,
        removed: RefCell<Vec<(u64, String)>>,
        comments: RefCell<Vec<(u64, String)>>,
        upserts: RefCell<Vec<(u64, String, String)>>,
    }

    impl IssueTracker for RecordingTracker {
        fn close(&self, _number: u64, _comment: &str) -> Result<()> {
            Ok(())
        }
        fn add_label(&self, number: u64, label: &str) -> Result<()> {
            self.added.borrow_mut().push((number, label.to_string()));
            Ok(())
        }
        fn remove_label(&self, number: u64, label: &str) -> Result<()> {
            self.removed.borrow_mut().push((number, label.to_string()));
            Ok(())
        }
        fn comment(&self, number: u64, body: &str) -> Result<()> {
            self.comments.borrow_mut().push((number, body.to_string()));
            Ok(())
        }
        fn upsert_marked_comment(&self, number: u64, marker: &str, body: &str) -> Result<()> {
            self.upserts
                .borrow_mut()
                .push((number, marker.to_string(), body.to_string()));
            Ok(())
        }
    }

    fn labels() -> TriageLabels {
        TriageLabels {
            queue_label: "ready-for-agent".into(),
            needs_info_label: NEEDS_INFO_LABEL.into(),
            triage_agent_label: TRIAGE_AGENT_LABEL.into(),
            marker: CONSOLIDATED_SPEC_MARKER.into(),
        }
    }

    #[test]
    fn promote_swaps_labels_without_comment() {
        let draft = TriageDraft {
            items: vec![TriageItem {
                number: 12,
                verdict: TriageVerdict::Promote,
                comment: None,
            }],
        };
        let t = RecordingTracker::default();
        apply_triage(&draft, &t, &labels(), |_| true).unwrap();
        assert_eq!(*t.removed.borrow(), vec![(12, "triage-agent".to_string())]);
        assert_eq!(*t.added.borrow(), vec![(12, "ready-for-agent".to_string())]);
        assert!(t.comments.borrow().is_empty(), "promote posts no comment");
        assert!(t.upserts.borrow().is_empty(), "promote upserts nothing");
    }

    #[test]
    fn consolidate_upserts_marked_comment_then_swaps_labels() {
        let body = format!("{CONSOLIDATED_SPEC_MARKER}\n## Consolidated spec\n...");
        let draft = TriageDraft {
            items: vec![TriageItem {
                number: 15,
                verdict: TriageVerdict::Consolidate,
                comment: Some(body.clone()),
            }],
        };
        let t = RecordingTracker::default();
        apply_triage(&draft, &t, &labels(), |_| true).unwrap();
        let upserts = t.upserts.borrow();
        assert_eq!(upserts.len(), 1);
        assert_eq!(upserts[0].0, 15);
        assert_eq!(upserts[0].1, CONSOLIDATED_SPEC_MARKER);
        assert!(upserts[0].2.contains("Consolidated spec"));
        // Label swap happened; no plain comment (the upsert is the comment).
        assert_eq!(*t.removed.borrow(), vec![(15, "triage-agent".to_string())]);
        assert_eq!(*t.added.borrow(), vec![(15, "ready-for-agent".to_string())]);
        assert!(t.comments.borrow().is_empty(), "consolidate uses upsert");
    }

    #[test]
    fn bounce_never_asks_and_swaps_to_needs_info() {
        let draft = TriageDraft {
            items: vec![TriageItem {
                number: 18,
                verdict: TriageVerdict::Bounce,
                comment: Some("Missing acceptance criteria.".into()),
            }],
        };
        let t = RecordingTracker::default();
        // `decide` panics if consulted — bounce must apply without it.
        apply_triage(&draft, &t, &labels(), |_| panic!("bounce must not ask")).unwrap();
        assert_eq!(
            *t.comments.borrow(),
            vec![(18, "Missing acceptance criteria.".to_string())]
        );
        assert_eq!(*t.removed.borrow(), vec![(18, "triage-agent".to_string())]);
        assert_eq!(*t.added.borrow(), vec![(18, "needs-info".to_string())]);
    }

    #[test]
    fn declined_confirmation_publishes_nothing() {
        let draft = TriageDraft {
            items: vec![
                TriageItem {
                    number: 1,
                    verdict: TriageVerdict::Promote,
                    comment: None,
                },
                TriageItem {
                    number: 2,
                    verdict: TriageVerdict::Consolidate,
                    comment: Some(format!("{CONSOLIDATED_SPEC_MARKER}\nspec")),
                },
            ],
        };
        let t = RecordingTracker::default();
        apply_triage(&draft, &t, &labels(), |_| false).unwrap();
        assert!(
            t.removed.borrow().is_empty(),
            "nothing published on decline"
        );
        assert!(t.added.borrow().is_empty());
        assert!(t.upserts.borrow().is_empty());
    }

    #[test]
    fn retriage_edits_existing_marked_comment() {
        // Idempotence lives behind `upsert_marked_comment`; this asserts the CLI
        // routes a consolidation through the upsert (never a plain `comment`), so a
        // re-triage edits the marked comment rather than stacking a second one.
        let draft = TriageDraft {
            items: vec![TriageItem {
                number: 7,
                verdict: TriageVerdict::Consolidate,
                comment: Some(format!("{CONSOLIDATED_SPEC_MARKER}\nv2 spec")),
            }],
        };
        let t = RecordingTracker::default();
        apply_triage(&draft, &t, &labels(), |_| true).unwrap();
        assert_eq!(t.upserts.borrow().len(), 1, "exactly one upsert");
        assert!(
            t.comments.borrow().is_empty(),
            "consolidation never posts a plain comment"
        );
    }
}
