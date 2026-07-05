//! Fetching cross-issue references (`## Blocked by` / `## Parent` sources) via
//! the `gh` CLI, for the planner's `.ralphy/references.md`.

use std::path::Path;

use anyhow::{Context, Result};

use crate::github::client::{gh, gh_output};

/// Parse `gh issue view --json number,title,state,body,url` into a [`Reference`].
fn parse_reference(json: &str) -> Result<crate::references::Reference> {
    #[derive(serde::Deserialize)]
    struct RefJson {
        number: u64,
        #[serde(default)]
        title: String,
        #[serde(default)]
        state: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        url: String,
    }
    let r: RefJson =
        serde_json::from_str(json).context("parsing `gh issue view` reference JSON")?;
    Ok(crate::references::Reference {
        number: r.number,
        state: r.state,
        title: r.title,
        body: r.body,
        url: r.url,
    })
}

/// Fetch a single issue's number, title, state, body, and URL via
/// `gh issue view <n> --json number,title,state,body,url` — the source a
/// structured reference (`## Blocked by` / `## Parent`) points at, reproduced for
/// the planner. The `url` is the handle the planner follows to pull the comment
/// thread on demand (this call omits comments by design). One call carries
/// everything `references.md` renders, distinct from the state-only
/// [`crate::github::issue_is_closed`] and the comments-only
/// [`crate::github::issue_comments`].
pub fn fetch_reference(number: u64, repo_root: &Path) -> Result<crate::references::Reference> {
    let out = gh_output(
        &format!("gh issue view {number} --json number,title,state,body,url"),
        || {
            let mut cmd = gh(repo_root);
            cmd.args([
                "issue",
                "view",
                &number.to_string(),
                "--json",
                "number,title,state,body,url",
            ]);
            cmd
        },
    )?;
    parse_reference(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// End-to-end evidence against the live issue
    /// `paulocorcino/bioledger-platform#29`: drive the real ralphy chain
    /// (`structured_refs` → `parse_reference` → `render_references_file`) over
    /// real GitHub data and print the `.ralphy/references.md` it produces.
    ///
    /// Ignored by default (needs network + `gh` auth + that specific repo). Run:
    ///   cargo test -p ralphy-core e2e_references_for_bioledger_29 -- --ignored --nocapture
    ///
    /// The only departure from production is the `--repo` flag here vs.
    /// `fetch_reference`'s cwd-pinned `gh`: the parser and renderer exercised are
    /// the exact ones the runner uses.
    #[test]
    #[ignore = "live network: paulocorcino/bioledger-platform"]
    fn e2e_references_for_bioledger_29() {
        const REPO: &str = "paulocorcino/bioledger-platform";

        // #29's real body, verbatim (the two structured sections are what matter).
        let body_29 = "## Parent\n\n\
            Part of PRD-0009. Split from #15 (Spike S2c — OCR + Redaction, bundle `needs-split`).\n\n\
            ## What to build\n\n\
            A metade Redaction do S2c, medida no harness sobre o corpus.\n\n\
            ## Blocked by\n\n\
            - #13 (S2a — corpus real + ground truth)\n";

        // 1. Our pure extractor over the real body: structured refs only.
        let refs = crate::blocked::structured_refs(body_29, 29);
        println!("\nstructured_refs(#29) = {refs:?}");
        assert_eq!(
            refs,
            vec![13, 15],
            "blocked-by (#13) leads, then parent (#15)"
        );

        // 2. Fetch each ref from the live repo and run OUR parser on the output.
        let mut fetched = Vec::new();
        for n in refs {
            let out = Command::new("gh")
                .args([
                    "issue",
                    "view",
                    &n.to_string(),
                    "--repo",
                    REPO,
                    "--json",
                    "number,title,state,body,url",
                ])
                .output()
                .expect("spawn gh");
            assert!(out.status.success(), "gh failed for #{n}");
            let r = parse_reference(&String::from_utf8_lossy(&out.stdout))
                .unwrap_or_else(|e| panic!("parse_reference(#{n}): {e}"));
            println!("  fetched #{} [{}] {}", r.number, r.state, r.title);
            fetched.push(r);
        }

        // 3. Our renderer produces the file the planner reads.
        let file =
            crate::references::render_references_file(&fetched).expect("non-empty references file");
        println!("\n----- .ralphy/references.md -----\n{file}\n---------------------------------");

        // Evidence assertions: both refs present with their real state, source
        // bodies reproduced — not paraphrased.
        assert!(file.contains("## #13 (CLOSED)"));
        assert!(file.contains("## #15 (CLOSED)"));
        assert!(file.contains("ground truth") || file.to_lowercase().contains("corpus"));
        assert!(file.contains("treat it as a lead"));
        // The source URL travels with each reference (the handle for comments).
        assert!(file.contains("/bioledger-platform/issues/13"));
        assert!(file.contains("/bioledger-platform/issues/15"));
    }

    #[test]
    fn parse_reference_reads_number_state_title_body() {
        let json = r#"{"number":13,"title":"S2a corpus","state":"OPEN","body":"ground truth corpus","url":"https://github.com/o/r/issues/13"}"#;
        let r = parse_reference(json).unwrap();
        assert_eq!(r.number, 13);
        assert_eq!(r.state, "OPEN");
        assert_eq!(r.title, "S2a corpus");
        assert!(r.body.contains("ground truth"));
        assert_eq!(r.url, "https://github.com/o/r/issues/13");
    }

    #[test]
    fn parse_reference_tolerates_missing_optional_fields() {
        let r = parse_reference(r#"{"number":7,"state":"CLOSED"}"#).unwrap();
        assert_eq!(r.number, 7);
        assert_eq!(r.state, "CLOSED");
        assert!(r.title.is_empty());
        assert!(r.body.is_empty());
        assert!(r.url.is_empty());
    }
}
