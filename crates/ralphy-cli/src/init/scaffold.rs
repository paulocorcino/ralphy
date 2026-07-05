use std::path::Path;

use anyhow::{Context, Result};

use super::wizard::InitConfig;

// The setup-pocock templates ship embedded in the binary so init has zero
// runtime dependency on the on-disk skills dir. Paths mirror the depth
// `ralphy-agent-claude/src/lib.rs` uses for `../../../assets/prompts/...`.
const TPL_ISSUE_GITHUB: &str =
    include_str!("../../../../assets/plugin/skills/setup-pocock/issue-tracker-github.md");
const TPL_ISSUE_GITLAB: &str =
    include_str!("../../../../assets/plugin/skills/setup-pocock/issue-tracker-gitlab.md");
const TPL_ISSUE_LOCAL: &str =
    include_str!("../../../../assets/plugin/skills/setup-pocock/issue-tracker-local.md");
const TPL_TRIAGE: &str =
    include_str!("../../../../assets/plugin/skills/setup-pocock/triage-labels.md");
const TPL_DOMAIN: &str = include_str!("../../../../assets/plugin/skills/setup-pocock/domain.md");
const TPL_ROADMAP: &str = include_str!("../../../../assets/plugin/skills/setup-pocock/roadmap.md");
const TPL_PRD_README: &str =
    include_str!("../../../../assets/plugin/skills/setup-pocock/prd-readme.md");
const TPL_PRD_TEMPLATE: &str =
    include_str!("../../../../assets/plugin/skills/setup-pocock/prd-template.md");

// SUSPENDED (see `write_scaffold`): the `## Agent skills` block write is parked
// under evaluation. These helpers are kept and unit-tested for re-enabling, so
// they read as dead in a non-test build — allow it rather than delete the work.
#[allow(dead_code)]
const AGENT_SKILLS_HEADING: &str = "## Agent skills";

/// Select the issue-tracker template by the remote host. A host containing
/// `gitlab` → the GitLab template, `github` → the GitHub template, anything else
/// (including `None`) → the local-markdown template. Returns the on-disk filename
/// to write (always `issue-tracker.md`) and the chosen template body. Pure over
/// its input.
fn select_issue_tracker(remote_host: Option<&str>) -> (&'static str, &'static str) {
    let host = remote_host.unwrap_or("").to_ascii_lowercase();
    let body = if host.contains("gitlab") {
        TPL_ISSUE_GITLAB
    } else if host.contains("github") {
        TPL_ISSUE_GITHUB
    } else {
        TPL_ISSUE_LOCAL
    };
    ("issue-tracker.md", body)
}

/// Render the `## Agent skills` block from the captured config: three one-line
/// summaries (issue tracker, triage labels, domain docs), each pointing at the
/// `docs/agents/*.md` file written alongside it.
#[allow(dead_code)] // SUSPENDED — see AGENT_SKILLS_HEADING.
fn agent_skills_block(cfg: &InitConfig) -> String {
    let tracker = match select_issue_tracker(cfg.remote_host.as_deref()).1 {
        b if b == TPL_ISSUE_GITHUB => "GitHub issues (use the `gh` CLI)",
        b if b == TPL_ISSUE_GITLAB => "GitLab issues (use the `glab` CLI)",
        _ => "local markdown files",
    };
    let mut out = String::new();
    out.push_str(AGENT_SKILLS_HEADING);
    out.push('\n');
    out.push_str(&format!(
        "\nThe engineering skills onboard from the docs below.\n\n\
         - Issue tracker: {tracker}. See `docs/agents/issue-tracker.md`.\n\
         - Triage labels: this repo's canonical triage roles. See `docs/agents/triage-labels.md`.\n\
         - Domain docs: single-context. See `docs/agents/domain.md`.\n"
    ));
    out
}

/// Replace an existing `## Agent skills` section in `doc` (from that heading up to
/// the next top-level `## ` heading or EOF) with `block`, or append `block` when
/// no such section exists. The result contains the heading exactly once. Pure.
#[allow(dead_code)] // SUSPENDED — see AGENT_SKILLS_HEADING.
fn upsert_agent_skills_block(doc: &str, block: &str) -> String {
    let block = block.trim_end();
    let Some(start) = find_heading(doc, AGENT_SKILLS_HEADING) else {
        // Append, separated by a blank line from any existing content.
        let mut out = doc.trim_end().to_string();
        if out.is_empty() {
            return format!("{block}\n");
        }

        out.push_str("\n\n");
        out.push_str(block);
        out.push('\n');
        return out;
    };

    // Find the end: the next h1/h2 heading after the section's body. Using any
    // top-level heading (not just `## `) as the boundary means a following `# `
    // or `## ` sibling section is preserved rather than silently clobbered; a
    // deeper `### ` nests under our section and is replaced with it.
    let after = &doc[start..];
    let body_offset = after.find('\n').map(|n| n + 1).unwrap_or(after.len());
    let end_rel = next_top_heading(&after[body_offset..]).map(|p| body_offset + p);

    let mut out = String::new();
    out.push_str(&doc[..start]);
    out.push_str(block);
    out.push('\n');
    if let Some(end_rel) = end_rel {
        out.push('\n');
        out.push_str(after[end_rel..].trim_start_matches('\n'));
    }

    out
}

/// Byte offset of the first line that starts with `needle` (at column 0), or
/// `None`. `needle` is matched as a line prefix so `## Agent skills` does not
/// match `### Agent skills sub`.
#[allow(dead_code)] // SUSPENDED — see AGENT_SKILLS_HEADING.
fn find_heading(doc: &str, needle: &str) -> Option<usize> {
    if doc.starts_with(needle) {
        return Some(0);
    }

    let pat = format!("\n{needle}");
    doc.find(&pat).map(|p| p + 1)
}

/// Byte offset of the first line that opens a top-level (h1 or h2) markdown
/// heading, or `None`. Used to bound an `## ` section: deeper `### ` headings
/// nest within it, so only `# ` / `## ` ends it.
#[allow(dead_code)] // SUSPENDED — see AGENT_SKILLS_HEADING.
fn next_top_heading(s: &str) -> Option<usize> {
    let mut offset = 0;
    for line in s.split_inclusive('\n') {
        let hashes = line.chars().take_while(|&c| c == '#').count();
        if (hashes == 1 || hashes == 2) && line[hashes..].starts_with(' ') {
            return Some(offset);
        }

        offset += line.len();
    }

    None
}

/// Write the deterministic scaffold onto the repo (ADR-0012 stage 5): the
/// `docs/agents/*` docs, the `## Agent skills` block in `CLAUDE.md`/`AGENTS.md`,
/// and — only when the dev opted in — the PRD/roadmap track docs. Idempotent:
/// every write overwrites in place and the block upsert never duplicates.
pub(crate) fn write_scaffold(repo: &Path, cfg: &InitConfig) -> Result<()> {
    let agents_dir = repo.join("docs").join("agents");
    std::fs::create_dir_all(&agents_dir).context("creating docs/agents")?;

    let (tracker_name, tracker_body) = select_issue_tracker(cfg.remote_host.as_deref());
    std::fs::write(agents_dir.join(tracker_name), tracker_body)
        .context("writing docs/agents/issue-tracker.md")?;
    std::fs::write(agents_dir.join("triage-labels.md"), TPL_TRIAGE)
        .context("writing docs/agents/triage-labels.md")?;
    std::fs::write(agents_dir.join("domain.md"), TPL_DOMAIN)
        .context("writing docs/agents/domain.md")?;

    // SUSPENDED (under evaluation): do not create or modify the repo's
    // CLAUDE.md/AGENTS.md. The `## Agent skills` block write is intentionally
    // disabled while we decide whether injecting it into the target repo is
    // necessary. The helpers (`agent_skills_block`/`upsert_agent_skills_block`)
    // are kept and still covered by unit tests; only the on-disk write here is
    // turned off. Re-enable by uncommenting the block below.
    //
    // // The block target: CLAUDE.md if present, else AGENTS.md if present, else a
    // // fresh CLAUDE.md.
    // let claude = repo.join("CLAUDE.md");
    // let agents = repo.join("AGENTS.md");
    // let target = if claude.exists() {
    //     claude
    // } else if agents.exists() {
    //     agents
    // } else {
    //     claude
    // };
    // let existing = std::fs::read_to_string(&target).unwrap_or_default();
    // let updated = upsert_agent_skills_block(&existing, &agent_skills_block(cfg));
    // std::fs::write(&target, updated).with_context(|| format!("writing {}", target.display()))?;

    if cfg.adopt_prd_roadmap {
        let prd_dir = repo.join("docs").join("prd");
        std::fs::create_dir_all(&prd_dir).context("creating docs/prd")?;
        std::fs::write(repo.join("docs").join("roadmap.md"), TPL_ROADMAP)
            .context("writing docs/roadmap.md")?;
        std::fs::write(prd_dir.join("README.md"), TPL_PRD_README)
            .context("writing docs/prd/README.md")?;
        std::fs::write(prd_dir.join("_template.md"), TPL_PRD_TEMPLATE)
            .context("writing docs/prd/_template.md")?;
    }

    Ok(())
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
    fn select_issue_tracker_picks_body_by_host() {
        assert!(select_issue_tracker(Some("github.com"))
            .1
            .contains("# Issue tracker: GitHub"));
        assert!(select_issue_tracker(Some("gitlab.com"))
            .1
            .contains("# Issue tracker: GitLab"));
        assert!(select_issue_tracker(None)
            .1
            .contains("# Issue tracker: Local Markdown"));
        // The on-disk filename is always issue-tracker.md regardless of host.
        assert_eq!(
            select_issue_tracker(Some("github.com")).0,
            "issue-tracker.md"
        );
    }

    fn block_cfg() -> InitConfig {
        config_of(&sample_report())
    }

    #[test]
    fn upsert_appends_block_when_absent() {
        let doc = "# Project\n\nSome intro.\n";
        let block = agent_skills_block(&block_cfg());
        let out = upsert_agent_skills_block(doc, &block);
        assert_eq!(
            out.matches("## Agent skills").count(),
            1,
            "exactly one heading:\n{out}"
        );
        assert!(
            out.contains("# Project"),
            "original content preserved:\n{out}"
        );
        assert!(
            out.trim_end().ends_with("docs/agents/domain.md`."),
            "block appended at end:\n{out}"
        );
    }

    #[test]
    fn upsert_replaces_existing_block_in_place() {
        let doc = "# Project\n\n## Agent skills\n\nOLD STALE BODY.\n\n## Other\n\nkeep me.\n";
        let block = agent_skills_block(&block_cfg());
        let out = upsert_agent_skills_block(doc, &block);
        assert_eq!(
            out.matches("## Agent skills").count(),
            1,
            "still exactly one heading:\n{out}"
        );
        assert!(!out.contains("OLD STALE BODY"), "old body gone:\n{out}");
        assert!(
            out.contains("docs/agents/issue-tracker.md"),
            "new summary present:\n{out}"
        );
        assert!(
            out.contains("## Other"),
            "trailing section preserved:\n{out}"
        );
        assert!(out.contains("keep me."), "trailing body preserved:\n{out}");
    }

    #[test]
    fn upsert_preserves_following_h1_sibling_section() {
        // Regression: a `# `/`## ` section after Agent skills must survive the
        // replace — only the section's own body (and any `### ` subsection) goes.
        let doc =
            "## Agent skills\n\nOLD BODY.\n\n### old sub\n\nnested old.\n\n# Top Level\n\nkeep me.\n";
        let block = agent_skills_block(&block_cfg());
        let out = upsert_agent_skills_block(doc, &block);
        assert_eq!(out.matches("## Agent skills").count(), 1);
        assert!(!out.contains("OLD BODY"), "old body gone:\n{out}");
        assert!(!out.contains("nested old"), "nested old sub gone:\n{out}");
        assert!(out.contains("# Top Level"), "h1 sibling preserved:\n{out}");
        assert!(out.contains("keep me."), "h1 body preserved:\n{out}");
    }

    #[test]
    fn write_scaffold_prd_opt_in_controls_prd_docs() {
        let dir = std::env::temp_dir().join(format!("ralphy-scaffold-prd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut cfg = block_cfg();
        cfg.adopt_prd_roadmap = true;
        write_scaffold(&dir, &cfg).unwrap();
        assert!(dir.join("docs/roadmap.md").exists());
        assert!(dir.join("docs/prd/README.md").exists());
        assert!(dir.join("docs/prd/_template.md").exists());

        let dir2 =
            std::env::temp_dir().join(format!("ralphy-scaffold-noprd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir2);
        std::fs::create_dir_all(&dir2).unwrap();
        let mut cfg2 = block_cfg();
        cfg2.adopt_prd_roadmap = false;
        write_scaffold(&dir2, &cfg2).unwrap();
        assert!(!dir2.join("docs/roadmap.md").exists());
        assert!(!dir2.join("docs/prd").exists());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }
}
