//! ADR-0042 D6's policy gate: Cursor uploads the enclosing repository as a side
//! effect of answering a question, so before Ralphy spawns it, every enclosing
//! repository must carry the opt-out.
//!
//! The rule is stated over the child's **working directory**, not the verb — the
//! indexing service is spawned by the CLI, so every invocation is covered:
//!
//! > Any `cursor` invocation whose cwd is inside a git repository must have
//! > `.cursorindexingignore` in that repository's root.
//!
//! Ralphy **creates** that file itself when it is missing and announces it on the
//! run log (`tracing::warn!`): a hard refusal stopped every Cursor run on the
//! operator, so instead the gate leaves a visible file in their `git status` they
//! can commit or delete, and an explicit opt-in turns the whole thing off. It is
//! not a silent write — the notice names the file, the tree it protects, and the
//! opt-in key.

//! The rule itself lives in `ralphy_proc_util::cursor` (ADR-0042 D19) so the
//! daemon's interactive launch enforces the SAME gate without importing the
//! core; this module is the run path's entry point onto it.

use std::path::Path;

/// D6's preflight. `Ok(())` when the child may be spawned — writing the opt-out
/// into any unprotected enclosing root first; `Err` only when that write fails.
///
/// Three ways to pass writing nothing: the operator opted in (`allow_indexing`),
/// the cwd is outside any repository, or every enclosing root already carries the
/// opt-out file.
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

    /// The contents Ralphy writes: exactly the one line that suppresses the tree.
    fn optout_body(dir: &Path) -> String {
        fs::read_to_string(dir.join(".cursorindexingignore")).expect("opt-out file")
    }

    #[test]
    fn indexing_gate_creates_the_optout_in_a_repo_without_one() {
        let d = repo();
        assert!(
            indexing_gate(d.path(), false).is_ok(),
            "the gate no longer refuses — it writes the opt-out and proceeds"
        );
        assert_eq!(optout_body(d.path()), "*\n");
    }

    #[test]
    fn indexing_gate_allows_with_the_optout_file() {
        let d = repo();
        fs::write(d.path().join(".cursorindexingignore"), "*\n").unwrap();
        assert!(indexing_gate(d.path(), false).is_ok());
    }

    /// The rule is about the repository ROOT, not the cwd: a run whose working
    /// directory is a nested subdirectory is still uploading the whole repository,
    /// so the opt-out lands at the ROOT even from a deep cwd.
    #[test]
    fn indexing_gate_creates_the_optout_at_the_root_from_a_nested_subdir() {
        let d = repo();
        let nested = d.path().join("crates").join("deep");
        fs::create_dir_all(&nested).unwrap();
        assert!(indexing_gate(&nested, false).is_ok());
        assert_eq!(
            optout_body(d.path()),
            "*\n",
            "the opt-out must be written at the ROOT, not the nested cwd"
        );
    }

    /// D6's measured evidence: a run indexed the PARENT repository, not the working
    /// directory it was given. So EVERY enclosing root must get the opt-out — an
    /// inner one alone would leave the outer tree uploading.
    #[test]
    fn indexing_gate_creates_the_optout_in_every_enclosing_repository() {
        let outer = repo();
        let inner = outer.path().join("vendor").join("nested");
        fs::create_dir_all(inner.join(".git")).unwrap();

        // Inner already opted out, outer not: the gate writes the OUTER one.
        fs::write(inner.join(".cursorindexingignore"), "*\n").unwrap();
        assert!(indexing_gate(&inner, false).is_ok());
        assert_eq!(
            optout_body(outer.path()),
            "*\n",
            "the outer tree is protected"
        );
        assert_eq!(optout_body(&inner), "*\n", "the inner opt-out is untouched");
    }

    /// D6 explicitly allows this: `draft_issues` / `consolidate_knowledge` may run
    /// where there is no repository, and the gate must not degrade into "refuse
    /// everything", which would make those verbs unreachable.
    #[test]
    fn indexing_gate_allows_when_there_is_no_repository_at_all() {
        let d = tempfile::tempdir().unwrap();
        assert!(indexing_gate(d.path(), false).is_ok());
    }

    /// The opt-in reaches the capability AND writes nothing: an operator who wants
    /// the indexing must not find an opt-out file suppressing it.
    #[test]
    fn the_opt_in_setting_writes_nothing_and_allows_indexing() {
        let d = repo();
        let before = listing(d.path());
        assert!(
            indexing_gate(d.path(), true).is_ok(),
            "the operator's explicit opt-in must reach the capability"
        );
        assert_eq!(
            listing(d.path()),
            before,
            "opt-in must NOT write the opt-out — that would suppress the indexing the operator asked for"
        );
    }

    /// The gate does not rewrite a tree that already carries the opt-out.
    #[test]
    fn the_gate_writes_nothing_when_already_protected() {
        let d = repo();
        fs::write(d.path().join(".cursorindexingignore"), "*\n").unwrap();
        let before = listing(d.path());
        indexing_gate(d.path(), false).unwrap();
        assert_eq!(
            listing(d.path()),
            before,
            "an already-protected tree must not be rewritten"
        );
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
