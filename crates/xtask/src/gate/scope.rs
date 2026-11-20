//! What the change under the gate touches. A change that edits only Markdown
//! outside `assets/` and `crates/` cannot change what the code steps check:
//! no build embeds those files and no test outside xtask reads them
//! (`include_str!` and `include_dir!` name `assets/` only). The gate skips
//! the code steps for such a change, and runs every other step.

use std::path::Path;

use crate::capabilities::git;

/// The steps whose inputs are the code under `crates/` and the Cargo files.
/// A name here that no step has fails `every_code_step_is_a_step`.
pub(super) const CODE_STEPS: &[&str] = &[
    "fmt",
    "clippy",
    "machete",
    "complexity",
    "doc",
    "ui-tests",
    "tsc",
    "oxlint",
    "oxlint-types",
    "oxfmt",
    "jscpd",
    "jscpd-report",
    "jscpd-count",
    "source-pins",
    "nextest",
    "doctest",
];

/// The branch a change is measured from, as the other steps measure it.
const BASE: &str = "main";

/// What the change touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Scope {
    /// Code, or anything the gate cannot rule out: every step runs.
    Everything,
    /// Only these Markdown files outside `assets/` and `crates/`.
    DocsOnly(Vec<String>),
}

/// Whether a changed path is Markdown that no build and no code test reads.
fn is_free_doc(path: &str) -> bool {
    let markdown = Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
    markdown && !path.starts_with("assets/") && !path.starts_with("crates/")
}

/// The scope of `paths`, the files the change touches.
pub(super) fn classify(paths: Vec<String>) -> Scope {
    if !paths.is_empty() && paths.iter().all(|p| is_free_doc(p)) {
        Scope::DocsOnly(paths)
    } else {
        Scope::Everything
    }
}

/// The files changed since the merge base with [`BASE`]: commits, the working
/// tree, and new files that are not ignored. A rename lists both of its paths.
fn changed_paths(root: &Path) -> anyhow::Result<Vec<String>> {
    let base = git(root, &["merge-base", BASE, "HEAD"])?;
    let tracked = git(root, &["diff", "--name-only", "--no-renames", base.trim()])?;
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard"])?;
    let mut paths: Vec<String> = tracked
        .lines()
        .chain(untracked.lines())
        .map(str::to_owned)
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// The scope of the change in `root`. When git cannot answer, every step runs,
/// and the reason is printed.
pub(super) fn detect(root: &Path) -> Scope {
    match changed_paths(root) {
        Ok(paths) => classify(paths),
        Err(e) => {
            println!("every step runs: the changed files are not known ({e:#})");
            Scope::Everything
        }
    }
}

/// Whether a step runs for a change of this scope.
pub(super) fn runs_for(scope: &Scope, step: &str) -> bool {
    match scope {
        Scope::Everything => true,
        Scope::DocsOnly(_) => !CODE_STEPS.contains(&step),
    }
}

#[cfg(test)]
mod tests;
