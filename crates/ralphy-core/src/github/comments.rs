//! Issue comment CRUD: fetching, posting, editing, and marker-based upsert of
//! comments on GitHub issues via the `gh` CLI.

use std::path::Path;

use anyhow::{Context, Result};

use crate::github::client::{gh, gh_output, gh_stdin};

/// Post a comment on a GitHub issue via `gh issue comment <n> --body <comment>`.
pub fn comment_issue(number: u64, comment: &str, repo_root: &Path) -> Result<()> {
    gh_output(&format!("gh issue comment {number}"), || {
        let mut cmd = gh(repo_root);
        cmd.args(["issue", "comment", &number.to_string(), "--body", comment]);
        cmd
    })?;
    Ok(())
}

/// Parse `{"comments":[{"body":"..."}]}` JSON from `gh issue view --json comments`
/// into the comment bodies, in thread order.
pub fn parse_issue_comments(json: &str) -> Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct CommentJson {
        body: String,
    }
    #[derive(serde::Deserialize)]
    struct CommentsJson {
        #[serde(default)]
        comments: Vec<CommentJson>,
    }
    let c: CommentsJson =
        serde_json::from_str(json).context("parsing `gh issue view --json comments`")?;
    Ok(c.comments.into_iter().map(|c| c.body).collect())
}

/// Fetch an issue's comment bodies via `gh issue view <n> --json comments`.
pub fn issue_comments(number: u64, repo_root: &Path) -> Result<Vec<String>> {
    let out = gh_output(&format!("gh issue view {number} --json comments"), || {
        let mut cmd = gh(repo_root);
        cmd.args(["issue", "view", &number.to_string(), "--json", "comments"]);
        cmd
    })?;
    parse_issue_comments(&String::from_utf8_lossy(&out.stdout))
}

/// Parse the REST `GET .../issues/{n}/comments` JSON array into `(id, body)`
/// pairs in thread order. Unlike [`parse_issue_comments`] this keeps the numeric
/// comment `id` — the handle a REST `PATCH .../issues/comments/{id}` needs to edit
/// a specific comment (the `gh issue view --json comments` node ids cannot drive
/// the REST edit). Comments with a null/absent body default to empty.
pub fn parse_rest_comments(json: &str) -> Result<Vec<(u64, String)>> {
    #[derive(serde::Deserialize)]
    struct RestComment {
        id: u64,
        #[serde(default)]
        body: String,
    }
    let comments: Vec<RestComment> =
        serde_json::from_str(json).context("parsing REST issue comments JSON")?;
    Ok(comments.into_iter().map(|c| (c.id, c.body)).collect())
}

/// Fetch an issue's comments WITH their numeric REST ids via
/// `gh api repos/{owner}/{repo}/issues/{n}/comments --paginate`. The `{owner}` /
/// `{repo}` placeholders resolve from the `repo_root` cwd. Used to find Ralphy's
/// own marked consolidated-spec comment for an idempotent edit (ADR-0017).
pub fn list_comments_with_ids(number: u64, repo_root: &Path) -> Result<Vec<(u64, String)>> {
    let path = format!("repos/{{owner}}/{{repo}}/issues/{number}/comments");
    let out = gh_output(&format!("gh api {path}"), || {
        let mut cmd = gh(repo_root);
        cmd.args(["api", &path, "--paginate"]);
        cmd
    })?;
    parse_rest_comments(&String::from_utf8_lossy(&out.stdout))
}

/// The numeric id of the first comment whose body carries `marker`, or `None`.
/// The seam that makes `ralphy triage`'s consolidated-spec comment idempotent:
/// found → edit that id; absent → post a fresh one.
pub fn find_marked_comment(comments: &[(u64, String)], marker: &str) -> Option<u64> {
    comments
        .iter()
        .find(|(_, body)| body.contains(marker))
        .map(|(id, _)| *id)
}

/// Edit an existing issue comment by numeric id via
/// `gh api repos/{owner}/{repo}/issues/comments/{id} -X PATCH --input -`, sending
/// `{"body": ...}` on stdin (never argv — bodies carry markdown, newlines, and
/// quotes that would break Windows quoting). Mirrors [`crate::github::edit_issue_body`]'s
/// stdin-pipe + transient-retry shape.
pub fn edit_comment(id: u64, body: &str, repo_root: &Path) -> Result<()> {
    let path = format!("repos/{{owner}}/{{repo}}/issues/comments/{id}");
    let payload = serde_json::json!({ "body": body }).to_string();
    gh_stdin(
        &format!("gh api comment edit ({id})"),
        payload.as_bytes(),
        || {
            let mut c = gh(repo_root);
            c.args(["api", &path, "-X", "PATCH", "--input", "-"]);
            c
        },
    )?;
    Ok(())
}

/// Post-or-edit the single comment carrying `marker` on an issue (ADR-0017): find
/// Ralphy's own marked comment and EDIT it, or post a fresh one when none exists.
/// Idempotent by construction — re-triage never stacks a second consolidated-spec
/// comment. The author's body and other people's comments are never touched.
pub fn upsert_marked_comment(
    number: u64,
    marker: &str,
    body: &str,
    repo_root: &Path,
) -> Result<()> {
    let existing = list_comments_with_ids(number, repo_root)?;
    match find_marked_comment(&existing, marker) {
        Some(id) => edit_comment(id, body, repo_root),
        None => comment_issue(number, body, repo_root),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rest_comments_extracts_ids_and_bodies() {
        let json = r#"[
            { "id": 111, "body": "first" },
            { "id": 222, "body": "<!-- ralphy:consolidated-spec -->\nspec" }
        ]"#;
        let got = parse_rest_comments(json).expect("parse");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], (111, "first".to_string()));
        assert_eq!(got[1].0, 222);
        assert!(got[1].1.contains("consolidated-spec"));
    }

    #[test]
    fn find_marked_comment_matches_marker_only() {
        let comments = vec![
            (1u64, "just chatter".to_string()),
            (
                2u64,
                "<!-- ralphy:consolidated-spec -->\nthe spec".to_string(),
            ),
            (
                3u64,
                "another <!-- ralphy:consolidated-spec --> later".to_string(),
            ),
        ];
        // First marked comment wins; unmarked comments are skipped.
        assert_eq!(
            find_marked_comment(&comments, "<!-- ralphy:consolidated-spec -->"),
            Some(2)
        );
        assert_eq!(
            find_marked_comment(&comments[..1], "<!-- ralphy:consolidated-spec -->"),
            None
        );
    }
}
