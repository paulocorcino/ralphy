//! The version of a CLI tool that CI downloads and a local command of
//! `AGENTS.md` runs: one constant, so the two run the same tool.

use super::*;

/// The oxlint of the `ui-tests` job of `ci.yml`, fetched by version and
/// checksum, and of the workbench lint command of `AGENTS.md`, run by `npx`.
const OXLINT_VERSION: &str = "1.85.0";

#[test]
fn ci_installs_the_pinned_oxlint() {
    let root = workspace_root();
    let ci = read(&root.join(".github/workflows/ci.yml"));
    assert!(
        ci.contains(&format!("OXLINT_VERSION: \"{OXLINT_VERSION}\"")),
        "ci.yml must pin oxlint {OXLINT_VERSION}"
    );
    let agents = read(&root.join("AGENTS.md"));
    let command = Regex::new(r"oxlint@(\S+)").expect("a valid literal regex");
    let versions: Vec<&str> = command
        .captures_iter(&agents)
        .map(|caps| caps.get(1).map_or("", |m| m.as_str()))
        .collect();
    assert!(
        !versions.is_empty(),
        "AGENTS.md must name the oxlint command of the workbench gate"
    );
    assert!(
        versions.iter().all(|v| *v == OXLINT_VERSION),
        "every oxlint command in AGENTS.md must run oxlint {OXLINT_VERSION}, found {versions:?}"
    );
}
