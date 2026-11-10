//! The gate runs `shellcheck` and `PSScriptAnalyzer` on files it names, not on a
//! folder. A script that is tracked but not named is never checked; a name
//! whose file is gone fails the step. The two lists must be equal.

use super::*;

/// The tracked files that match `pattern`, as `git ls-files` prints them.
fn tracked(pattern: &str) -> BTreeSet<String> {
    let out = std::process::Command::new("git")
        .args(["ls-files", pattern])
        .current_dir(workspace_root())
        .output()
        .expect("git runs in the workspace");
    assert!(out.status.success(), "git ls-files {pattern} failed");
    String::from_utf8(out.stdout)
        .expect("git prints file names as UTF-8")
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The paths that end in `.ext` among the arguments of the gate step `name`.
fn named_by_step(name: &str, ext: &str) -> BTreeSet<String> {
    let step = gate_steps::STEPS.iter().find(|s| s.name == name);
    assert!(step.is_some(), "the gate has no step named {name}");
    let args = step.map_or(&[][..], |s| s.args);
    let re = Regex::new(&format!(r"[A-Za-z0-9_./-]+\.{ext}\b")).expect("a fixed pattern");
    args.iter()
        .flat_map(|arg| re.find_iter(arg).map(|m| m.as_str().to_owned()))
        .collect()
}

fn assert_same(step: &str, ext: &str, pattern: &str) {
    let tracked = tracked(pattern);
    assert!(
        !tracked.is_empty(),
        "the scan is blind: no tracked {pattern}"
    );
    let named = named_by_step(step, ext);
    let missing: Vec<&String> = tracked.difference(&named).collect();
    let stale: Vec<&String> = named.difference(&tracked).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "the `{step}` step of crates/xtask/src/gate/steps.rs must name every tracked {pattern} \
         file, and only those. Not named: {missing:?}. Named, not tracked: {stale:?}"
    );
}

#[test]
fn the_gate_shellcheck_step_names_every_tracked_shell_script() {
    assert_same("shellcheck", "sh", "*.sh");
}

#[test]
fn the_gate_psscriptanalyzer_step_names_every_tracked_powershell_script() {
    assert_same("psscriptanalyzer", "ps1", "*.ps1");
}
