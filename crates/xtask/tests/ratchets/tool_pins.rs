//! The version of a CLI tool that CI downloads and a step of `xtask gate`
//! runs: one constant, so the two run the same tool.

use super::*;

/// The versions that the steps of `xtask gate` give `tool` as an argument
/// `<tool>@<version>`.
fn gate_versions(tool: &str) -> Vec<&'static str> {
    let prefix = format!("{tool}@");
    gate_steps::STEPS
        .iter()
        .flat_map(|s| s.args.iter())
        .filter_map(|arg| arg.strip_prefix(prefix.as_str()))
        .collect()
}

/// Fails unless `ci.yml` pins `env_key` at `version`, and every step of the
/// gate that runs `tool` runs that version.
fn assert_pinned(env_key: &str, tool: &str, version: &str) {
    let ci = read(&workspace_root().join(".github/workflows/ci.yml"));
    assert!(
        ci.contains(&format!("{env_key}: \"{version}\"")),
        "ci.yml must pin {tool} {version}"
    );
    let versions = gate_versions(tool);
    assert!(
        !versions.is_empty(),
        "a step of crates/xtask/src/gate/steps.rs must run {tool}"
    );
    assert!(
        versions.iter().all(|v| *v == version),
        "every {tool} step of the gate must run {tool} {version}, found {versions:?}"
    );
}

/// The oxlint of the `ui-tests` job of `ci.yml`, fetched by version and
/// checksum, and of the lint steps of the gate, run by `npx`.
const OXLINT_VERSION: &str = "1.93.0";

#[test]
fn ci_installs_the_pinned_oxlint() {
    assert_pinned("OXLINT_VERSION", "oxlint", OXLINT_VERSION);
}

/// The tsgolint of the type-aware run of the `ui-tests` job of `ci.yml`,
/// fetched by version and checksum, and of the type-aware step of the gate.
const TSGOLINT_VERSION: &str = "7.0.2006";

#[test]
fn ci_installs_the_pinned_tsgolint() {
    assert_pinned("TSGOLINT_VERSION", "oxlint-tsgolint", TSGOLINT_VERSION);
}

/// The oxfmt of the format check of the `ui-tests` job of `ci.yml`, fetched
/// by version and checksum, and of the format step of the gate. A new version
/// formats some text in another way, so a raise is its own reformat commit.
const OXFMT_VERSION: &str = "0.75.0";

#[test]
fn ci_installs_the_pinned_oxfmt() {
    assert_pinned("OXFMT_VERSION", "oxfmt", OXFMT_VERSION);
}

/// The `channel` of `rust-toolchain.toml`.
fn pinned_rust_toolchain() -> String {
    let file = read(&workspace_root().join("rust-toolchain.toml"));
    file.lines()
        .find_map(|line| line.trim().strip_prefix("channel = "))
        .map(|v| v.trim_matches('"').to_owned())
        .expect("rust-toolchain.toml has a `channel = \"<version>\"` line")
}

/// Every `toolchain:` of every workflow is the version of
/// `rust-toolchain.toml`, so CI builds with the compiler the gate ran.
#[test]
fn ci_uses_the_pinned_rust_toolchain() {
    let pinned = pinned_rust_toolchain();
    assert!(
        pinned.split('.').count() == 3,
        "rust-toolchain.toml pins an exact version, found {pinned:?}"
    );
    let dir = workspace_root().join(".github/workflows");
    let mut wrong = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("reading the workflows folder") {
        let path = entry.expect("a workflow entry").path();
        for (n, line) in read(&path).lines().enumerate() {
            let Some(value) = line.trim().strip_prefix("toolchain: ") else {
                continue;
            };
            if value.trim_matches('"') != pinned {
                wrong.push(format!("{}:{}: {value}", path.display(), n + 1));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "a workflow must install Rust {pinned}: {wrong:?}"
    );
}

/// The ast-grep of the `checks` job of `ci.yml`, fetched by version and
/// checksum, and of the ast-grep steps of the gate, run by `npx`.
const ASTGREP_VERSION: &str = "0.51.1";

#[test]
fn ci_installs_the_pinned_ast_grep() {
    assert_pinned("ASTGREP_VERSION", "@ast-grep/cli", ASTGREP_VERSION);
}
