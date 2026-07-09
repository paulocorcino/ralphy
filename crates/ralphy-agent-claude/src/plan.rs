//! Planning-side assets and helpers: the embedded planning/consolidation prompts,
//! the bundled Claude Code plugin and its materialization, planning-prompt
//! selection for an issue, and the `.ralphy/plan-charter.md` writeout. The
//! `plan()` entry point itself stays on `impl Agent for ClaudeAgent` in `lib.rs`.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use include_dir::{include_dir, Dir};
use ralphy_core::{Issue, Workspace};

/// The planning prompt, embedded so the binary is self-contained as a global
/// tool. Copied to `.ralphy/plan-charter.md` for the live session to read;
/// only a one-line pointer is piped on stdin. Single source of truth lives at
/// `assets/prompts/`.
pub(crate) const PROMPT_PLAN: &str = include_str!("../../../assets/prompts/prompt.plan.md");

/// The staged-plan planning prompt, used when the issue carries the
/// `stagedplan` label.
pub(crate) const PROMPT_PLAN_STAGED: &str =
    include_str!("../../../assets/prompts/prompt.plan.staged.md");

/// The operational Claude Code plugin, embedded at build time so the binary is a
/// self-contained global tool. It bundles the `reviewer` and `staged-plan`
/// skills the planning/execution prompts depend on; the single source of truth
/// lives at the repo root under `assets/plugin`.
static PLUGIN: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../assets/plugin");

/// `<repo>/.ralphy/plugin` — where this adapter materializes its embedded
/// Claude Code plugin. Derived from the vendor-neutral `ralphy_dir()`; the
/// core does not know the subdir exists (ADR-0002 amendment, #79).
fn plugin_dir(ws: &Workspace) -> PathBuf {
    ws.ralphy_dir().join("plugin")
}

/// Materialize the embedded plugin into the workspace's `.ralphy/plugin` so it
/// can be handed to `claude` via `--plugin-dir`. Re-extracted from scratch each
/// call (the tree is tiny) so a stale or partly-written copy never lingers, and
/// the run never depends on whatever skills are installed globally. Returns the
/// plugin directory to pass on the command line.
pub(crate) fn materialize_plugin(ws: &Workspace) -> Result<PathBuf> {
    let dir = plugin_dir(ws);
    ralphy_adapter_support::materialize_assets(&PLUGIN, &dir, None)?;
    Ok(dir)
}

/// Select the planning prompt for an issue. Returns `(prompt, staged)` where
/// `staged` is `true` when the issue carries the `stagedplan` label.
pub(crate) fn plan_prompt_for(issue: &Issue) -> (&'static str, bool) {
    if issue.labels.iter().any(|l| l == "stagedplan") {
        (PROMPT_PLAN_STAGED, true)
    } else {
        (PROMPT_PLAN, false)
    }
}

/// Write the selected planning charter to `.ralphy/plan-charter.md` (mirrors
/// `.ralphy/exec.md`); rewritten each plan call so a resumed session and a
/// `stagedplan` label switch both see the right content.
pub(crate) fn write_plan_charter(ws: &Workspace, prompt: &str) -> Result<()> {
    fs::write(ws.plan_charter_path(), prompt).context("writing .ralphy/plan-charter.md")
}

/// The env var a staged plan sets so the `staged-plan` skill knows it is running
/// non-interactively (no TTY to prompt on). Returns `Some((key, value))` when
/// `staged`, otherwise `None` so the standard plan leaves the environment clean.
pub(crate) fn staged_plan_env(staged: bool) -> Option<(&'static str, &'static str)> {
    if staged {
        Some(("STAGED_PLAN_NONINTERACTIVE", "1"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue_with_labels(labels: &[&str]) -> Issue {
        Issue {
            number: 1,
            title: "test".into(),
            body: String::new(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
            comments: vec![],
        }
    }

    /// The charter file must carry whichever prompt `plan_prompt_for` selected,
    /// and a rewrite (as every `plan()` call performs) must replace a staged
    /// charter with the standard one after a label switch.
    #[test]
    fn plan_charter_file_carries_selected_prompt() {
        let base = std::env::temp_dir().join(format!("ralphy-plan-charter-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);
        fs::create_dir_all(ws.ralphy_dir()).unwrap();

        let (staged_prompt, _) = plan_prompt_for(&issue_with_labels(&["stagedplan"]));
        write_plan_charter(&ws, staged_prompt).expect("write staged charter");
        assert_eq!(
            fs::read_to_string(ws.plan_charter_path()).unwrap(),
            PROMPT_PLAN_STAGED,
            "staged issue must put the staged charter on disk"
        );

        let (standard_prompt, _) = plan_prompt_for(&issue_with_labels(&["bug"]));
        write_plan_charter(&ws, standard_prompt).expect("rewrite standard charter");
        assert_eq!(
            fs::read_to_string(ws.plan_charter_path()).unwrap(),
            PROMPT_PLAN,
            "the per-call rewrite must replace a stale staged charter"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// The per-issue stdin payload must stay a one-line pointer, not regrow
    /// into the full charter — pins the byte reduction issue #80 delivers.
    #[test]
    fn plan_pointer_is_a_pointer_not_the_charter() {
        let pointer = ralphy_adapter_support::PLAN_CHARTER;
        assert!(pointer.len() * 50 < PROMPT_PLAN.len());
        assert!(pointer.len() * 50 < PROMPT_PLAN_STAGED.len());
    }

    #[test]
    fn plan_prompts_carry_finalize_trailer() {
        // Pin the FULL literal (suffix + spacing), not just the prefix: a drift to
        // `issue = <N> -->` would keep a prefix check green yet make the trailer no
        // longer match `plan_is_finalized_for`, silently disabling resume.
        assert!(
            PROMPT_PLAN.contains("<!-- ralphy-plan: issue=<N> -->"),
            "standard plan prompt must instruct writing the exact finalized-plan trailer"
        );
        assert!(
            PROMPT_PLAN_STAGED.contains("<!-- ralphy-plan: issue=<N> -->"),
            "staged plan prompt must instruct writing the exact finalized-plan trailer"
        );
    }

    #[test]
    fn plan_prompt_for_selects_staged_when_label_present() {
        let issue = issue_with_labels(&["bug", "stagedplan"]);
        let (prompt, staged) = plan_prompt_for(&issue);
        assert!(staged, "should be staged when 'stagedplan' label present");
        assert_eq!(
            prompt, PROMPT_PLAN_STAGED,
            "should use the staged plan prompt"
        );
    }

    #[test]
    fn materialize_plugin_extracts_required_skills() {
        // The embedded plugin must carry the skills the prompts invoke; a run
        // provisions them into .ralphy/plugin so claude finds them via
        // --plugin-dir without depending on globally-installed skills.
        let base = std::env::temp_dir().join(format!("ralphy-plugin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let ws = Workspace::new(&base);

        let dir = materialize_plugin(&ws).expect("materialize");
        assert_eq!(dir, plugin_dir(&ws));
        assert!(
            dir.join(".claude-plugin/plugin.json").is_file(),
            "plugin manifest must be materialized"
        );
        assert!(
            dir.join("skills/reviewer/SKILL.md").is_file(),
            "reviewer skill must be materialized"
        );
        assert!(
            dir.join("skills/staged-plan/SKILL.md").is_file(),
            "staged-plan skill must be materialized"
        );
        // Multi-file skills must come through whole, not just the SKILL.md.
        assert!(
            dir.join("skills/reviewer/scripts/audit.py").is_file(),
            "reviewer helper scripts must be materialized"
        );

        // Idempotent: a second call clears and re-extracts cleanly.
        materialize_plugin(&ws).expect("re-materialize");
        assert!(dir.join("skills/reviewer/SKILL.md").is_file());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn plan_prompt_for_selects_standard_without_label() {
        let issue = issue_with_labels(&["bug", "ready-for-agent"]);
        let (prompt, staged) = plan_prompt_for(&issue);
        assert!(
            !staged,
            "should not be staged when 'stagedplan' label absent"
        );
        assert_eq!(prompt, PROMPT_PLAN, "should use the standard plan prompt");
    }

    #[test]
    fn plan_prompt_for_not_staged_with_no_labels() {
        let issue = issue_with_labels(&[]);
        let (_, staged) = plan_prompt_for(&issue);
        assert!(!staged);
    }

    #[test]
    fn staged_plan_env_set_when_staged() {
        assert_eq!(
            staged_plan_env(true),
            Some(("STAGED_PLAN_NONINTERACTIVE", "1")),
            "a staged plan must flag the skill as non-interactive"
        );
    }

    #[test]
    fn staged_plan_env_absent_when_not_staged() {
        assert_eq!(
            staged_plan_env(false),
            None,
            "the standard plan must not touch the environment"
        );
    }

    #[test]
    fn staged_prompt_and_skill_reference_noninteractive_flag() {
        // Anti-drift: the env the adapter sets is only meaningful if the staged
        // prompt (and the skill it invokes) still read STAGED_PLAN_NONINTERACTIVE.
        assert!(
            PROMPT_PLAN_STAGED.contains("STAGED_PLAN_NONINTERACTIVE"),
            "staged plan prompt must reference the non-interactive flag it is handed"
        );
        let skill = include_str!("../../../assets/plugin/skills/staged-plan/SKILL.md");
        assert!(
            skill.contains("STAGED_PLAN_NONINTERACTIVE"),
            "staged-plan skill must read the non-interactive flag"
        );
    }
}
