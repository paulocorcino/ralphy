use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use ralphy_core::{github, DraftRequest, IssuesDraft, Workspace};

use super::gate::Agent;
use super::wizard::{InitConfig, InitState};

/// Which judgment path stage 8 takes for the captured config — or `Skip` when the
/// diagnosis/Q&A found no backlog or milestone (ADR-0012 stage 8 "skipped cleanly"
/// criterion). Pure: the impure shell acts on this verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IssuesPath {
    Milestone,
    LooseBacklog,
    Skip,
}

/// Decide the stage-8 path from the captured config. The milestone path wins when
/// the dev adopted the PRD/roadmap model AND milestone docs exist; otherwise a
/// loose backlog is reshaped when one was found; otherwise stage 8 is skipped.
pub(crate) fn decide_issues_path(cfg: &InitConfig) -> IssuesPath {
    if cfg.adopt_prd_roadmap && !cfg.milestone_docs.is_empty() {
        IssuesPath::Milestone
    } else if cfg.backlog_location.is_some() {
        IssuesPath::LooseBacklog
    } else {
        IssuesPath::Skip
    }
}

/// The draft decision for the task-drafting step. The prompt shows `[Y/n]`, so the
/// recommended default (empty/yes/y) drafts a preview — nothing is published, this
/// is a read-only agent call — and only an explicit decline skips it. Pure, mirrors
/// [`labels_decision`].
pub(crate) fn draft_decision(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

/// The publish decision for the drafted preview. Default is **No** (`[y/N]`): a
/// bulk external write is never the silent default — only an explicit `y`/`yes`
/// proceeds. Pure, mirrors [`download_decision`].
pub(crate) fn publish_decision(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// A human-readable summary of the draft for the dev to confirm before any
/// external write: the headline count (and milestone, when present) followed by
/// one line per issue with its labels and blocked-by indices. Pure, mirrors
/// [`github::format_label_plan`].
pub(crate) fn format_draft_summary(draft: &IssuesDraft) -> String {
    let mut out = String::new();
    match &draft.milestone {
        Some(ms) => out.push_str(&format!(
            "will create {} issue(s), all in milestone \"{}\"\n",
            draft.issue_count(),
            ms.title
        )),
        None => out.push_str(&format!("will create {} issue(s)\n", draft.issue_count())),
    }

    if let Some(prd) = &draft.prd_path {
        out.push_str(&format!("PRD written: {prd}\n"));
    }

    for (i, issue) in draft.issues.iter().enumerate() {
        let labels = if issue.labels.is_empty() {
            String::new()
        } else {
            format!("  [{}]", issue.labels.join(", "))
        };
        let blocked = if issue.blocked_by.is_empty() {
            String::new()
        } else {
            let refs: Vec<String> = issue
                .blocked_by
                .iter()
                .map(|n| format!("#{}", n + 1))
                .collect();
            format!("  (blocked by {})", refs.join(", "))
        };
        out.push_str(&format!(
            "  {}. {}{}{}\n",
            i + 1,
            issue.title,
            labels,
            blocked
        ));
    }

    out
}

/// Rewrite a drafted body's `## Blocked by` placeholder with the resolved issue
/// numbers (or the "can start immediately" line when none). The charter emits the
/// literal `BLOCKED_BY_PLACEHOLDER`; if it is absent (a body that didn't follow
/// the template) the body is returned unchanged. Pure.
fn patch_blocked_by(body: &str, blocked_numbers: &[u64]) -> String {
    const PLACEHOLDER: &str = "BLOCKED_BY_PLACEHOLDER";

    if !body.contains(PLACEHOLDER) {
        return body.to_string();
    }

    let replacement = if blocked_numbers.is_empty() {
        "None - can start immediately".to_string()
    } else {
        blocked_numbers
            .iter()
            .map(|n| format!("- #{n}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    body.replace(PLACEHOLDER, &replacement)
}

/// Resolve the agent-ready triage label from the scaffolded
/// `docs/agents/triage-labels.md` (the mapping for `ready-for-agent`), falling
/// back to the canonical `ready-for-agent` when no mapping is configured.
pub(crate) fn resolve_triage_label(repo: &Path) -> String {
    let triage_doc = std::fs::read_to_string(repo.join("docs/agents/triage-labels.md")).ok();
    triage_doc
        .as_deref()
        .and_then(|d| github::parse_triage_mapping(d, "ready-for-agent"))
        .unwrap_or_else(|| "ready-for-agent".to_string())
}

/// Resolve the human-return label an `escalate` verdict swaps in (ADR-0018 §3)
/// from the scaffolded `docs/agents/triage-labels.md` mapping for
/// `ready-for-human`, falling back to the canonical `ready-for-human` when no
/// mapping is configured. The `HITL` alias is honored downstream by the runner's
/// human-gate classification, so no alias handling is needed at swap time.
pub(crate) fn resolve_human_label(repo: &Path) -> String {
    let triage_doc = std::fs::read_to_string(repo.join("docs/agents/triage-labels.md")).ok();
    triage_doc
        .as_deref()
        .and_then(|d| github::parse_triage_mapping(d, "ready-for-human"))
        .unwrap_or_else(|| "ready-for-human".to_string())
}

/// Publish a confirmed draft, threading the [`InitState`] checkpoint so a crash
/// mid-publish never recreates an already-published issue or milestone on resume
/// (ADR-0012 stage 9). The closures inject the external writes (`create_milestone`,
/// `create_issue`) and the persist step (`save`), mirroring `install_skills_step`.
///
/// INVARIANT (held at every step): each issue/milestone is created AT MOST ONCE
/// across runs. The milestone is created only when `state.milestone_created` is
/// `None`; issues are created only beyond `state.created_issues.len()` (the
/// persisted prefix). `save` runs after EVERY create, so a crash leaves a prefix
/// the next run resumes PAST, never before — `created_issues[i]` is the published
/// number of `draft.issues[i]`, so a later issue's `blocked_by` index resolves
/// against that prefix.
fn publish_draft_with(
    draft: &IssuesDraft,
    state: &mut InitState,
    mut save: impl FnMut(&InitState) -> Result<()>,
    mut create_milestone: impl FnMut(&str, &str) -> Result<u64>,
    mut create_issue: impl FnMut(&str, &str, &[String], Option<&str>) -> Result<u64>,
) -> Result<()> {
    // Create the milestone first (so `gh issue create --milestone <name>` resolves)
    // — but only once across runs. Each issue links to it by name.
    if let Some(ms) = &draft.milestone {
        if state.milestone_created.is_none() {
            let number = create_milestone(&ms.title, &ms.description)?;
            println!("  created milestone #{number}: {}", ms.title);
            state.milestone_created = Some(ms.title.clone());
            save(state)?;
        }
    }

    let milestone_name = draft.milestone.as_ref().map(|ms| ms.title.as_str());

    // Resume past the persisted prefix: the first `created_issues.len()` draft
    // entries are already on GitHub.
    for issue in draft.issues.iter().skip(state.created_issues.len()) {
        // A blocker index must point at an earlier (already-created) issue; the
        // persisted prefix makes this resolve on resume. Guard an out-of-range
        // index rather than panicking on a bad draft.
        let blocked_numbers: Vec<u64> = issue
            .blocked_by
            .iter()
            .filter_map(|&idx| state.created_issues.get(idx).copied())
            .collect();
        let body = patch_blocked_by(&issue.body, &blocked_numbers);
        let number = create_issue(&issue.title, &body, &issue.labels, milestone_name)?;
        println!("  created #{number}: {}", issue.title);
        state.created_issues.push(number);
        save(state)?;
    }

    Ok(())
}

/// Reload a persisted [`IssuesDraft`] from `issues-draft.json` — the draft a
/// prior run's `created_issues` prefix corresponds to. Used on a partial-publish
/// resume so the remainder publishes against the SAME draft, never a regenerated
/// one (which could reorder the prefix and duplicate a published issue).
pub(crate) fn load_issues_draft(path: &Path) -> Result<IssuesDraft> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

/// The impure wrapper: wire [`publish_draft_with`]'s closures to the real
/// `github::` POSTs and persist the checkpoint to `.ralphy/init-state.json` after
/// each create.
pub(crate) fn publish_draft(
    repo: &Path,
    draft: &IssuesDraft,
    state: &mut InitState,
    ws: &Workspace,
) -> Result<()> {
    publish_draft_with(
        draft,
        state,
        |s| s.save(ws),
        |title, description| github::create_milestone(repo, title, description),
        |title, body, labels, milestone| github::create_issue(repo, title, body, labels, milestone),
    )
}

/// Dispatch the backlog/milestone → issues draft session to the selected agent's
/// adapter. Like [`diagnose_with_agent`], the charter is shared
/// ([`ralphy_core::build_init_issues_prompt`]) and only the invocation differs.
pub(crate) fn draft_with_agent(
    agent: Agent,
    repo: &Path,
    out_path: &Path,
    req: &DraftRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<IssuesDraft> {
    match agent {
        Agent::Claude => {
            ralphy_agent_claude::draft_issues(repo, out_path, req, model, effort, timeout)
        }

        Agent::Codex => {
            ralphy_agent_codex::draft_issues(repo, out_path, req, model, effort, timeout)
        }

        Agent::Opencode => {
            ralphy_agent_opencode::draft_issues(repo, out_path, req, model, effort, timeout)
        }
    }
}

#[cfg(test)]
mod tests {
    use ralphy_core::{DiagnosisReport, RepoKind};

    use super::*;

    fn sample_report() -> DiagnosisReport {
        DiagnosisReport {
            repo_kind: RepoKind::Existing,
            language_build: Some("Rust / cargo".into()),
            backlog_location: Some("docs/backlog.md".into()),
            milestone_docs: vec!["docs/roadmap.md".into(), "docs/prd/0001.md".into()],
            skills_dir: Some(".claude".into()),
            has_context_or_adrs: true,
            remote_host: Some("github.com".into()),
        }
    }

    fn config_of(report: &DiagnosisReport) -> InitConfig {
        InitConfig {
            repo_kind: report.repo_kind,
            language_build: report.language_build.clone(),
            backlog_location: report.backlog_location.clone(),
            milestone_docs: report.milestone_docs.clone(),
            skills_dir: report.skills_dir.clone(),
            has_context_or_adrs: report.has_context_or_adrs,
            remote_host: report.remote_host.clone(),
            adopt_prd_roadmap: !report.milestone_docs.is_empty(),
        }
    }

    #[test]
    fn decide_issues_path_milestone_backlog_and_skip() {
        let mut cfg = config_of(&sample_report());
        // Milestone path: opted in AND milestone docs present.
        cfg.adopt_prd_roadmap = true;
        assert_eq!(decide_issues_path(&cfg), IssuesPath::Milestone);

        // Not opted in but a backlog exists → loose-backlog path.
        cfg.adopt_prd_roadmap = false;
        cfg.backlog_location = Some("docs/backlog.md".into());
        assert_eq!(decide_issues_path(&cfg), IssuesPath::LooseBacklog);

        // Neither milestone (opt-in off) nor backlog → skip cleanly.
        cfg.backlog_location = None;
        assert_eq!(decide_issues_path(&cfg), IssuesPath::Skip);

        // Opted in but NO milestone docs, with a backlog → loose-backlog, not milestone.
        cfg.adopt_prd_roadmap = true;
        cfg.milestone_docs = vec![];
        cfg.backlog_location = Some("BACKLOG.md".into());
        assert_eq!(decide_issues_path(&cfg), IssuesPath::LooseBacklog);
    }

    #[test]
    fn draft_decision_empty_and_yes_proceed_no_declines() {
        // Default-Yes: silence accepts the `[Y/n]` default and drafts.
        assert!(draft_decision(""));
        assert!(draft_decision("y"));
        assert!(draft_decision("  YES "));
        assert!(!draft_decision("n"));
        assert!(!draft_decision("no"));
        assert!(!draft_decision("nah"));
    }

    #[test]
    fn publish_decision_only_yes_proceeds() {
        assert!(publish_decision("y"));
        assert!(publish_decision("yes"));
        assert!(publish_decision("  YES "));
        // Default-No: silence and anything else declines.
        assert!(!publish_decision(""));
        assert!(!publish_decision("n"));
        assert!(!publish_decision("maybe"));
    }

    fn sample_draft() -> IssuesDraft {
        IssuesDraft {
            milestone: Some(ralphy_core::MilestoneDraft {
                title: "v1".into(),
                description: "first".into(),
            }),
            prd_path: Some("docs/prd/0001.md".into()),
            issues: vec![
                ralphy_core::IssueDraft {
                    title: "slice one".into(),
                    body: "## Blocked by\n\nBLOCKED_BY_PLACEHOLDER\n".into(),
                    labels: vec!["ready-for-agent".into()],
                    blocked_by: vec![],
                },
                ralphy_core::IssueDraft {
                    title: "slice two".into(),
                    body: "## Blocked by\n\nBLOCKED_BY_PLACEHOLDER\n".into(),
                    labels: vec!["ready-for-agent".into()],
                    blocked_by: vec![0],
                },
            ],
        }
    }

    #[test]
    fn format_draft_summary_reports_count_milestone_and_blocked_by() {
        let summary = format_draft_summary(&sample_draft());
        assert!(summary.contains("2 issue(s)"), "count:\n{summary}");
        assert!(
            summary.contains("milestone \"v1\""),
            "milestone:\n{summary}"
        );
        assert!(summary.contains("docs/prd/0001.md"), "prd:\n{summary}");
        assert!(summary.contains("slice two"), "issue title:\n{summary}");
        // blocked_by index 0 → 1-based "#1" in the human summary.
        assert!(
            summary.contains("blocked by #1"),
            "blocked-by ref:\n{summary}"
        );
    }

    #[test]
    fn patch_blocked_by_replaces_placeholder_and_handles_empty() {
        let body = "## Blocked by\n\nBLOCKED_BY_PLACEHOLDER\n";
        // With blockers: real refs.
        let patched = patch_blocked_by(body, &[7, 9]);
        assert!(patched.contains("- #7"));
        assert!(patched.contains("- #9"));
        assert!(!patched.contains("BLOCKED_BY_PLACEHOLDER"));
        // No blockers: the "can start immediately" line.
        let none = patch_blocked_by(body, &[]);
        assert!(none.contains("None - can start immediately"));
        assert!(!none.contains("BLOCKED_BY_PLACEHOLDER"));
        // Absent placeholder: returned unchanged.
        let plain = "no placeholder here";
        assert_eq!(patch_blocked_by(plain, &[1]), plain);
    }

    fn three_issue_draft() -> IssuesDraft {
        let issue = |title: &str, blocked_by: Vec<usize>| ralphy_core::IssueDraft {
            title: title.into(),
            body: "## Blocked by\n\nBLOCKED_BY_PLACEHOLDER\n".into(),
            labels: vec!["ready-for-agent".into()],
            blocked_by,
        };
        IssuesDraft {
            milestone: Some(ralphy_core::MilestoneDraft {
                title: "v1".into(),
                description: "first".into(),
            }),
            prd_path: None,
            issues: vec![
                issue("slice one", vec![]),
                issue("slice two", vec![0]),
                issue("slice three", vec![1]),
            ],
        }
    }

    #[test]
    fn publish_draft_with_never_recreates_persisted_prefix() {
        // Resume case: two issues + the milestone already published. Only the 3rd
        // issue is created; the milestone is NOT recreated.
        let draft = three_issue_draft();
        let mut state = InitState {
            created_issues: vec![101, 102],
            milestone_created: Some("v1".into()),
            ..InitState::default()
        };

        let mut ms_calls = 0;
        let mut issue_titles: Vec<String> = Vec::new();
        let mut save_calls = 0;
        publish_draft_with(
            &draft,
            &mut state,
            |_s| {
                save_calls += 1;
                Ok(())
            },
            |_t, _d| {
                ms_calls += 1;
                Ok(999)
            },
            |title, body, _labels, milestone| {
                issue_titles.push(title.to_string());
                // The 3rd issue is blocked_by index 1 → must resolve to the
                // persisted #102, proving blocked-by resolves against the prefix.
                assert!(body.contains("- #102"), "blocked-by resolved:\n{body}");
                assert_eq!(milestone, Some("v1"));
                Ok(103)
            },
        )
        .unwrap();

        assert_eq!(issue_titles, vec!["slice three".to_string()]);
        assert_eq!(ms_calls, 0, "milestone must NOT be recreated");
        assert_eq!(state.created_issues, vec![101, 102, 103]);
        assert!(save_calls >= 1, "save must fire after the create");

        // Fresh case: nothing published yet. Milestone created once, all 3 issues
        // created in order, numbers accumulate.
        let draft = three_issue_draft();
        let mut state = InitState::default();
        let mut ms_calls = 0;
        let mut next = 200u64;
        let mut issue_titles: Vec<String> = Vec::new();
        let mut save_calls = 0;
        publish_draft_with(
            &draft,
            &mut state,
            |_s| {
                save_calls += 1;
                Ok(())
            },
            |_t, _d| {
                ms_calls += 1;
                Ok(1)
            },
            |title, _body, _labels, milestone| {
                issue_titles.push(title.to_string());
                assert_eq!(milestone, Some("v1"));
                let n = next;
                next += 1;
                Ok(n)
            },
        )
        .unwrap();

        assert_eq!(ms_calls, 1, "milestone created exactly once");
        assert_eq!(
            issue_titles,
            vec![
                "slice one".to_string(),
                "slice two".to_string(),
                "slice three".to_string()
            ]
        );
        assert_eq!(state.created_issues, vec![200, 201, 202]);
        assert_eq!(state.milestone_created.as_deref(), Some("v1"));
        // save after milestone + after each of 3 issues.
        assert_eq!(save_calls, 4);
    }

    #[test]
    fn load_issues_draft_round_trips_persisted_draft() {
        // The partial-publish resume path reloads this exact file instead of
        // regenerating, so it must parse what publish writes.
        let dir = std::env::temp_dir().join(format!("ralphy-draft-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::new(&dir);
        std::fs::create_dir_all(ws.ralphy_dir()).unwrap();

        let draft = three_issue_draft();
        let path = ws.issues_draft_path();
        std::fs::write(&path, serde_json::to_string_pretty(&draft).unwrap()).unwrap();

        let back = load_issues_draft(&path).unwrap();
        assert_eq!(back, draft);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
