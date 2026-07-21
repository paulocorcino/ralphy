//! ADR-0042 D6's policy gate: Cursor uploads the enclosing repository as a side
//! effect of answering a question, so Ralphy refuses to spawn it in a repository
//! that has not opted out.
//!
//! The rule is stated over the child's **working directory**, not the verb — the
//! indexing service is spawned by the CLI, so every invocation is covered:
//!
//! > Any `cursor` invocation whose cwd is inside a git repository requires
//! > `.cursorindexingignore` in that repository's root.
//!
//! Ralphy never writes the file: disabling a vendor's data flow inside the
//! operator's own repository is their decision, and an unexplained new file in
//! their `git status` is not Ralphy's to leave. The gate only READS.

//! The rule itself lives in `ralphy_proc_util::cursor` (ADR-0042 D19) so the
//! daemon's interactive launch enforces the SAME refusal without importing the
//! core; this module is the run path's entry point onto it.

use std::path::Path;

/// D6's preflight. `Ok(())` when the child may be spawned; `Err` with an
/// actionable ADR-0013 stop otherwise.
///
/// Three ways to pass: the operator opted in (`allow_indexing`), the cwd is
/// outside any repository, or the repository root carries the opt-out file.
pub(crate) fn indexing_gate(work_dir: &Path, allow_indexing: bool) -> anyhow::Result<()> {
    ralphy_proc_util::cursor::indexing_gate(work_dir, allow_indexing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A temp directory that LOOKS like a git repository to the walk above.
    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        fs::create_dir(d.path().join(".git")).expect("mkdir .git");
        d
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn indexing_gate_refuses_a_repo_without_the_optout() {
        let d = repo();
        let err = indexing_gate(d.path(), false)
            .expect_err("a repository with no opt-out must refuse the spawn");
        let msg = err.to_string();
        // The message must be actionable, not merely a refusal: it names the file,
        // its one-line content, and the key that overrides it.
        assert!(msg.contains(".cursorindexingignore"), "{msg}");
        assert!(msg.contains('*'), "{msg}");
        assert!(
            msg.contains("cursor.allow_codebase_indexing_i_understand_the_risk"),
            "{msg}"
        );
    }

    #[test]
    fn indexing_gate_allows_with_the_optout_file() {
        let d = repo();
        fs::write(d.path().join(".cursorindexingignore"), "*\n").unwrap();
        assert!(indexing_gate(d.path(), false).is_ok());
    }

    /// The rule is about the repository ROOT, not the cwd: a run whose working
    /// directory is a nested subdirectory is still uploading the whole repository.
    #[test]
    fn indexing_gate_resolves_the_root_from_a_nested_subdir() {
        let d = repo();
        let nested = d.path().join("crates").join("deep");
        fs::create_dir_all(&nested).unwrap();
        assert!(
            indexing_gate(&nested, false).is_err(),
            "a nested cwd must resolve the enclosing root"
        );
        fs::write(d.path().join(".cursorindexingignore"), "*\n").unwrap();
        assert!(
            indexing_gate(&nested, false).is_ok(),
            "the opt-out at the ROOT covers a nested cwd"
        );
    }

    /// D6's measured evidence: a run indexed the PARENT repository, not the working
    /// directory it was given. So an opt-out in an inner repository alone must not
    /// pass — the outer tree is what would be uploaded.
    #[test]
    fn indexing_gate_requires_the_optout_in_every_enclosing_repository() {
        let outer = repo();
        let inner = outer.path().join("vendor").join("nested");
        fs::create_dir_all(inner.join(".git")).unwrap();

        // Inner opted out, outer not: still refused, and the message names the OUTER
        // root — the larger tree, and the one the operator has to protect.
        fs::write(inner.join(".cursorindexingignore"), "*\n").unwrap();
        let err = indexing_gate(&inner, false)
            .expect_err("an inner opt-out must not cover the enclosing repository");
        assert!(
            err.to_string()
                .contains(&outer.path().display().to_string()),
            "{err}"
        );

        // Both opted out: allowed.
        fs::write(outer.path().join(".cursorindexingignore"), "*\n").unwrap();
        assert!(indexing_gate(&inner, false).is_ok());
    }

    /// D6 explicitly allows this: `draft_issues` / `consolidate_knowledge` may run
    /// where there is no repository, and the gate must not degrade into "refuse
    /// everything", which would make those verbs unreachable.
    #[test]
    fn indexing_gate_allows_when_there_is_no_repository_at_all() {
        let d = tempfile::tempdir().unwrap();
        assert!(indexing_gate(d.path(), false).is_ok());
    }

    #[test]
    fn the_opt_in_setting_overrides_the_refusal() {
        let d = repo();
        assert!(indexing_gate(d.path(), false).is_err());
        assert!(
            indexing_gate(d.path(), true).is_ok(),
            "the operator's explicit opt-in must reach the capability"
        );
    }

    /// D6: Ralphy never creates the opt-out file, and the gate is a pure read on
    /// BOTH paths — the refusing one and the allowing one.
    #[test]
    fn the_gate_writes_nothing() {
        let d = repo();
        let before = listing(d.path());
        let _ = indexing_gate(d.path(), false);
        assert_eq!(listing(d.path()), before, "the refusal must write nothing");

        fs::write(d.path().join(".cursorindexingignore"), "*\n").unwrap();
        let before = listing(d.path());
        indexing_gate(d.path(), false).unwrap();
        assert_eq!(listing(d.path()), before, "the pass must write nothing too");
    }

    /// D6: the sibling ignore file denies the vendor's edit tool, so Ralphy must
    /// never write it, require it, or even name it. Fragments are assembled with
    /// `concat!` so this assertion cannot match ITSELF — and the scan runs over
    /// every source file in the crate, not just this one.
    #[test]
    fn no_cursorignore_anywhere_in_the_crate() {
        // Recursive: an ADR-0022 `foo.rs` + `foo/` split must not silently drop a
        // file out of this scan.
        fn scan(dir: &Path, needle: &str, hits: &mut Vec<String>) {
            for entry in fs::read_dir(dir).expect("src/ is readable") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    scan(&path, needle, hits);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                // This test's own two fragments are the only legitimate occurrences,
                // and they are never contiguous — so any hit is a real one.
                if fs::read_to_string(&path)
                    .expect("read source")
                    .contains(needle)
                {
                    hits.push(path.display().to_string());
                }
            }
        }
        let needle = concat!(".cursor", "ignore");
        let mut hits = Vec::new();
        scan(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
            needle,
            &mut hits,
        );
        assert!(
            hits.is_empty(),
            "the plain ignore file breaks the vendor's edit tool (D6); found in {hits:?}"
        );
    }
}
