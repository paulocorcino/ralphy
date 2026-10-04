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

    // The gate's own tests are in `ralphy_proc_util::cursor`, next to the rule.

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
