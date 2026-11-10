//! The gate-exit ratchet: a CI step or a gate step that passes when its tool
//! fails hides a defect. The sites that do it only go down.

use super::*;

/// The lines in the workflows, the `.github` scripts and the gate steps that
/// let a command fail without failing the step: `|| true`, `|| :`,
/// `|| exit 0`, `continue-on-error: true`, `command -v` and `set +e`. Lower it
/// in the same change that removes one: the count is exact, so it cannot grow
/// back.
const GATE_EXITS: usize = 5;

/// `file:line: text` of each such line.
fn exit_sites() -> Vec<String> {
    let pattern =
        Regex::new(r"\|\|\s*(true\b|:|exit 0)|continue-on-error:\s*true|command -v |set \+e")
            .expect("a fixed pattern");
    let root = workspace_root();
    let github = root.join(".github");
    let mut files = Vec::new();
    rust_coupling::collect_all(&github, &mut files);
    files.retain(|p| p.extension().is_some_and(|e| e == "yml" || e == "sh"));
    skipped_tests::rust_sources(&root.join("crates/xtask/src/gate"), &mut files);
    assert!(
        files.len() > 5,
        "expected the workflows and the gate steps under {}, found {} files",
        root.display(),
        files.len()
    );
    let mut sites = Vec::new();
    for file in &files {
        let rel = rust_coupling::rel_path(&root, file);
        for (i, line) in read(file).lines().enumerate() {
            if pattern.is_match(line) {
                sites.push(format!("{rel}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    sites
}

#[test]
fn gate_exits_do_not_grow() {
    let sites = exit_sites();
    assert_eq!(
        sites.len(),
        GATE_EXITS,
        "{} lines let a step pass when its tool fails, the baseline is {GATE_EXITS}: \
         make the step fail with its tool; a removed line lowers GATE_EXITS in the same \
         change:\n{}",
        sites.len(),
        sites.join("\n")
    );
}
