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

/// A glob of `taplo.toml` as a regex: `**/` is any run of folders, `*` stays in
/// one folder.
fn glob_regex(glob: &str) -> Regex {
    let one_folder = |part: &str| -> String {
        part.chars()
            .map(|c| match c {
                '*' => "[^/]*".to_owned(),
                _ => regex::escape(&c.to_string()),
            })
            .collect()
    };
    let parts: Vec<String> = glob.split("**/").map(one_folder).collect();
    Regex::new(&format!("^{}$", parts.join("(?:[^/]+/)*")))
        .expect("a taplo glob is a valid pattern")
}

/// The `include` globs of `taplo.toml`.
fn taplo_includes() -> Vec<String> {
    let text = read(&workspace_root().join("taplo.toml"));
    let (_, rest) = text
        .split_once("include =")
        .expect("taplo.toml has an include list");
    let list = rest.split(']').next().unwrap_or_default();
    let quoted = Regex::new(r#""([^"]+)""#).expect("a fixed pattern");
    quoted
        .captures_iter(list)
        .map(|c| c[1].to_owned())
        .collect()
}

#[test]
fn taplo_reads_every_tracked_toml_file() {
    let includes: Vec<Regex> = taplo_includes().iter().map(|g| glob_regex(g)).collect();
    let missed: Vec<String> = tracked("*.toml")
        .into_iter()
        .filter(|f| !includes.iter().any(|re| re.is_match(f)))
        .collect();
    assert!(
        missed.is_empty(),
        "taplo.toml `include` does not reach these tracked TOML files; add their folder: {missed:?}"
    );
}

#[test]
fn a_taplo_glob_keeps_a_star_in_one_folder() {
    assert!(glob_regex("crates/*/Cargo.toml").is_match("crates/ralphy-cli/Cargo.toml"));
    assert!(!glob_regex("*.toml").is_match("docs/x.toml"));
    assert!(glob_regex("docs/lint/**/*.toml").is_match("docs/lint/a/b/clippy.toml"));
    assert!(glob_regex("docs/lint/**/*.toml").is_match("docs/lint/clippy.toml"));
}

#[test]
fn yamllint_reads_every_tracked_yaml_file() {
    let step = gate_steps::STEPS.iter().find(|s| s.name == "yamllint");
    let paths: Vec<&str> = step
        .map_or(&[][..], |s| s.args)
        .iter()
        .skip(3)
        .copied()
        .collect();
    assert!(
        !paths.is_empty(),
        "the gate has a yamllint step that names its paths"
    );
    let mut yaml = tracked("*.yml");
    yaml.extend(tracked("*.yaml"));
    let missed: Vec<String> = yaml
        .into_iter()
        .filter(|f| {
            !paths
                .iter()
                .any(|p| f == p || f.starts_with(&format!("{p}/")))
        })
        .collect();
    assert!(
        missed.is_empty(),
        "the yamllint step (and its CI step) does not name these tracked YAML files: {missed:?}"
    );
}
