//! Inlining the run's `.ralphy/` artifacts onto the Gemini child's stdin (#275).
//!
//! The Gemini CLI honours the repo `.gitignore` for its `read_file`/glob tools,
//! and Ralphy gitignores `.ralphy/` with `*` on purpose (see [`crate::root`]). So
//! every `.ralphy/…` path the execute charter tells the child to read is REFUSED
//! (`invalid_tool_params: … is ignored by configured ignore patterns`). A capable
//! model shell-`cat`s around it; the weakest flash cannot, and loops re-routing
//! around its own run inputs until the per-issue budget is spent (found live in
//! the #265 capstone, filed as #275).
//!
//! The fix hands the child the CONTENT instead of a PATH: the execute charter
//! still names `.ralphy/plan.md` and friends, but their bytes ride on stdin under
//! the same names, so no gitignored disk read is needed to converge. This mirrors
//! D12, where the planner already writes `.ralphy/plan.md` itself rather than
//! trusting the vendor's plan mode.

use ralphy_core::Workspace;

/// The execute charter's own artifacts, in the charter's reading order: the plan
/// (the source of truth, re-read every turn) and the issue first, then the retry
/// briefs the runner drops between attempts, then the predecessor context.
///
/// `knowledge/` is deliberately absent: it is an unbounded, cross-run cache whose
/// charter read is advisory, and inlining it whole would risk the vendor's 8 MiB
/// stdin ceiling for context the child rarely needs. A capable model can still
/// shell-`cat` it; a weak one converging on its plan does not miss it.
const EXEC_ARTIFACTS: &[&str] = &[
    "plan.md",
    "issue.json",
    "verify-failure.md",
    "protocol-failure.md",
    "handoffs.md",
    "references.md",
    "environment.md",
];

/// What separates the charter from the inlined bytes, and one artifact from the
/// next. Fenced with a plain-text banner rather than a Markdown code fence because
/// the artifacts (`plan.md`, the briefs) contain their own triple-backtick fences,
/// which would close a Markdown wrapper early.
const PREAMBLE: &str = "\n\n\
    ===== INLINED RUN CONTEXT (delivered on stdin) =====\n\
    The `.ralphy/` directory is gitignored, so the Gemini CLI refuses to read the \
    run artifacts the charter above refers to. Their exact contents are delivered \
    below instead. Treat each block as the verbatim file it names — do NOT try to \
    read it from disk, and prefer this content over any stale copy.\n";

/// Assemble the execute-phase stdin: the charter, then the content of every
/// [`EXEC_ARTIFACTS`] file that currently exists under `.ralphy/`.
pub(crate) fn exec_stdin(charter: &str, ws: &Workspace) -> String {
    let dir = ws.ralphy_dir();
    let present: Vec<(String, String)> = EXEC_ARTIFACTS
        .iter()
        .filter_map(|name| {
            let body = std::fs::read_to_string(dir.join(name)).ok()?;
            Some((format!(".ralphy/{name}"), body))
        })
        .collect();
    assemble(charter, &present)
}

/// Pure over its inputs so the framing is asserted without a filesystem. An empty
/// `files` (nothing on disk yet — the very first attempt before any artifact) is
/// the untouched charter, so the stdin the vendor sees is byte-identical to today
/// when there is nothing to inline.
fn assemble(charter: &str, files: &[(String, String)]) -> String {
    if files.is_empty() {
        return charter.to_string();
    }
    let mut out = String::from(charter);
    out.push_str(PREAMBLE);
    for (name, body) in files {
        out.push_str("\n===== BEGIN ");
        out.push_str(name);
        out.push_str(" =====\n");
        out.push_str(body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("===== END ");
        out.push_str(name);
        out.push_str(" =====\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_on_disk_leaves_the_charter_untouched() {
        // The first attempt, before any artifact exists, must pipe exactly what it
        // pipes today — the inlining is additive, never a rewrite of the charter.
        assert_eq!(assemble("the charter", &[]), "the charter");
    }

    #[test]
    fn each_present_artifact_is_fenced_under_its_ralphy_name() {
        let files = vec![
            (".ralphy/plan.md".to_string(), "- [ ] step one".to_string()),
            (
                ".ralphy/issue.json".to_string(),
                "{\"number\":7}".to_string(),
            ),
        ];
        let out = assemble("CHARTER", &files);
        assert!(out.starts_with("CHARTER"));
        assert!(out.contains(
            "===== BEGIN .ralphy/plan.md =====\n- [ ] step one\n===== END .ralphy/plan.md ====="
        ));
        assert!(out.contains("===== BEGIN .ralphy/issue.json =====\n{\"number\":7}\n===== END .ralphy/issue.json ====="));
        // The plan comes before the issue: the charter's reading order.
        assert!(out.find("plan.md").unwrap() < out.find("issue.json").unwrap());
    }

    #[test]
    fn a_body_with_its_own_backtick_fence_is_not_prematurely_closed() {
        // plan.md carries ```md blocks; a Markdown wrapper would break on them, so
        // the banner fence must survive an artifact that contains triple backticks.
        let body = "notes\n```rust\nfn x() {}\n```\ndone";
        let files = vec![(".ralphy/plan.md".to_string(), body.to_string())];
        let out = assemble("C", &files);
        assert!(out.contains("===== END .ralphy/plan.md ====="));
        assert!(out.contains("fn x() {}"));
    }

    #[test]
    fn only_present_files_appear() {
        // The retry briefs are absent on a first attempt; their labels must not be
        // conjured when the file was never written.
        let files = vec![(".ralphy/plan.md".to_string(), "p".to_string())];
        let out = assemble("C", &files);
        assert!(!out.contains("protocol-failure.md"));
        assert!(!out.contains("verify-failure.md"));
    }

    #[test]
    fn exec_stdin_inlines_what_the_workspace_holds() {
        let dir = tempfile::tempdir().unwrap();
        let ralphy = dir.path().join(".ralphy");
        std::fs::create_dir_all(&ralphy).unwrap();
        std::fs::write(ralphy.join("plan.md"), "- [ ] do it\n").unwrap();
        std::fs::write(ralphy.join("issue.json"), "{\"number\":9}\n").unwrap();

        let ws = Workspace::new(dir.path());
        let out = exec_stdin("CHARTER", &ws);
        assert!(out.starts_with("CHARTER"));
        assert!(out.contains("===== BEGIN .ralphy/plan.md ====="));
        assert!(out.contains("- [ ] do it"));
        assert!(out.contains("{\"number\":9}"));
        // A file that was never written contributes nothing.
        assert!(!out.contains("environment.md"));
    }
}
