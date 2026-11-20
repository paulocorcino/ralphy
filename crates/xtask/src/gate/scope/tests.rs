use std::path::Path;
use std::process::Command;

use super::*;
use crate::gate::steps::STEPS;

fn paths(list: &[&str]) -> Vec<String> {
    list.iter().map(ToString::to_string).collect()
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=gate",
            "-c",
            "user.email=gate@example.invalid",
        ])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// A repository on `main` with one commit of a Rust file and a doc.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    run_git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(dir.path().join("crates/a/src")).unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::write(dir.path().join("crates/a/src/lib.rs"), "fn a() {}\n").unwrap();
    std::fs::write(dir.path().join("docs/a.md"), "# A\n").unwrap();
    run_git(dir.path(), &["add", "."]);
    run_git(dir.path(), &["commit", "-q", "-m", "first"]);
    dir
}

#[test]
fn markdown_outside_assets_and_crates_is_docs_only() {
    let docs = paths(&["CONTEXT.md", "docs/adr/0091-x.md", "docs/NOTES.MD"]);
    assert_eq!(classify(docs.clone()), Scope::DocsOnly(docs));
}

#[test]
fn code_embedded_markdown_or_no_change_runs_everything() {
    for list in [
        &[][..],
        &["docs/a.md", "crates/xtask/src/gate.rs"][..],
        &["assets/prompts/prompt.plan.md"][..],
        &["crates/ralphy-cli/README.md"][..],
        &["docs/lint/scripts/tally.py"][..],
    ] {
        assert_eq!(classify(paths(list)), Scope::Everything, "{list:?}");
    }
}

#[test]
fn every_code_step_is_a_step() {
    for name in CODE_STEPS {
        assert!(
            STEPS.iter().any(|s| s.name == *name),
            "no step is named {name}"
        );
    }
}

#[test]
fn a_docs_only_scope_leaves_out_only_the_code_steps() {
    let docs = Scope::DocsOnly(paths(&["docs/a.md"]));
    assert!(!runs_for(&docs, "nextest"));
    assert!(runs_for(&docs, "markdownlint"));
    assert!(runs_for(&docs, "ratchets"), "the ratchets read the docs");
    assert!(runs_for(&Scope::Everything, "nextest"));
}

#[test]
fn detect_sees_edited_and_new_docs_in_the_working_tree() {
    let dir = repo();
    std::fs::write(dir.path().join("docs/a.md"), "# A, edited\n").unwrap();
    std::fs::write(dir.path().join("docs/b.md"), "# B\n").unwrap();
    assert_eq!(
        detect(dir.path()),
        Scope::DocsOnly(paths(&["docs/a.md", "docs/b.md"]))
    );
}

#[test]
fn detect_sees_a_code_file_that_moved_into_docs() {
    let dir = repo();
    run_git(dir.path(), &["mv", "crates/a/src/lib.rs", "docs/lib.md"]);
    assert_eq!(detect(dir.path()), Scope::Everything);
}
