//! Repository and milestone creation via the `gh` CLI (ADR-0012 stage 8), used
//! by `ralphy init`'s bootstrap — distinct from issue CRUD.

use std::path::Path;

use anyhow::{Context, Result};

use crate::github::client::{gh, gh_output};

/// Parse the milestone number from `gh api .../milestones` JSON (the created
/// milestone object carries a `number`).
fn parse_milestone_number(json: &str) -> Result<u64> {
    #[derive(serde::Deserialize)]
    struct MilestoneJson {
        number: u64,
    }
    let m: MilestoneJson =
        serde_json::from_str(json).context("parsing `gh api .../milestones` JSON")?;
    Ok(m.number)
}

/// Create a GitHub repository from the local repo at `repo_root` via
/// `gh repo create`, wiring `origin` to the new remote and pushing the current
/// branch. `name` is the new repo's name (the bootstrap derives it from the
/// directory); `private` selects visibility. Used by `ralphy init`'s bootstrap to
/// give a freshly `git init`-ed directory the GitHub remote the environment gate
/// requires. The local repo must already have a commit (see
/// [`crate::git::initial_commit`]) so `--push` has something to send.
pub fn create_repo(repo_root: &Path, name: &str, private: bool) -> Result<()> {
    let visibility = if private { "--private" } else { "--public" };
    gh_output(&format!("gh repo create {name}"), || {
        let mut cmd = gh(repo_root);
        cmd.args([
            "repo", "create", name, "--source", ".", "--remote", "origin", "--push", visibility,
        ]);
        cmd
    })?;
    Ok(())
}

/// Create a GitHub Milestone via `gh api repos/{owner}/{repo}/milestones` (the
/// `{owner}`/`{repo}` placeholders are resolved by `gh` from the repo dir). Returns
/// the created milestone's number, which [`crate::github::create_issue`] links
/// issues to. ADR-0012 stage 8 (milestone path).
pub fn create_milestone(repo_root: &Path, title: &str, description: &str) -> Result<u64> {
    let out = gh_output(&format!("gh api milestones (create {title})"), || {
        let mut cmd = gh(repo_root);
        cmd.args([
            "api",
            "--method",
            "POST",
            "repos/{owner}/{repo}/milestones",
            "-f",
            &format!("title={title}"),
            "-f",
            &format!("description={description}"),
        ]);
        cmd
    })?;
    parse_milestone_number(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_milestone_number_reads_number_field() {
        let json = r#"{"number": 3, "title": "v1", "state": "open"}"#;
        assert_eq!(parse_milestone_number(json).unwrap(), 3);
    }
}
