//! The steps of the local gate: the local form of each check step of
//! `ci.yml` and of the test jobs of `release.yml`. The ratchet
//! `the_gate_mirrors_every_ci_step` compiles this file too and fails when a
//! CI step has no step here, or a step here names a CI step that is gone.

/// The `program` of a step that runs xtask itself: the gate starts its own
/// program, so a step never has to build xtask while it runs.
pub const XTASK: &str = "xtask";

/// One check that the gate runs.
pub struct Step {
    /// The short name the table prints, and the name of the log file.
    pub name: &'static str,
    /// The names of the CI steps this step is the local form of.
    pub ci: &'static [&'static str],
    pub program: &'static str,
    pub args: &'static [&'static str],
    pub env: &'static [(&'static str, &'static str)],
    /// A program that must be on `PATH` besides `program`: the binary of a
    /// cargo subcommand, which cargo reports as an error, not as missing.
    pub needs: Option<&'static str>,
    /// `--fast` skips it: the workspace tests and the doctests.
    pub slow: bool,
}

const fn step(
    name: &'static str,
    ci: &'static [&'static str],
    program: &'static str,
    args: &'static [&'static str],
) -> Step {
    Step {
        name,
        ci,
        program,
        args,
        env: &[],
        needs: None,
        slow: false,
    }
}

/// The steps, quick ones first, so a fast failure is seen before the suite.
pub const STEPS: &[Step] = &[
    step(
        "fmt",
        &["Format check"],
        "cargo",
        &["fmt", "--all", "--check"],
    ),
    step(
        "clippy",
        &["Clippy"],
        "cargo",
        &[
            "clippy",
            "--locked",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    ),
    step(
        "ui-copy",
        &["UI copy (ADR-0065)"],
        XTASK,
        &["ui-copy", "--check"],
    ),
    step(
        "oversized",
        &["No file or line is over its limit"],
        XTASK,
        &["oversized", "--check"],
    ),
    step(
        "complexity",
        &["No Rust function over its complexity limit"],
        XTASK,
        &["complexity", "--check", "--base", "main"],
    ),
    step(
        "changelog",
        &["The fragments parse"],
        XTASK,
        &["changelog", "--check"],
    ),
    step("actionlint", &["Workflows (actionlint)"], "actionlint", &[]),
    step(
        "machete",
        &["Unused dependencies (cargo machete)"],
        "cargo-machete",
        &[],
    ),
    step("typos", &["Spelling (typos)"], "typos", &[]),
    Step {
        env: &[("RUSTDOCFLAGS", "-D warnings")],
        ..step(
            "doc",
            &["Rustdoc"],
            "cargo",
            &["doc", "--workspace", "--no-deps", "--locked"],
        )
    },
    step(
        "ui-tests",
        &["UI tests"],
        "node",
        &["--test", "crates/ralphy-daemon/ui-tests"],
    ),
    step(
        "tsc",
        &["UI type check (tsc)"],
        "npx",
        &[
            "-y",
            "-p",
            "typescript@7.0.2",
            "tsc",
            "--noEmit",
            "-p",
            "crates/ralphy-daemon/assets/ui",
        ],
    ),
    step(
        "oxlint",
        &["UI lint (oxlint)"],
        "npx",
        &[
            "-y",
            "oxlint@1.93.0",
            "--deny-warnings",
            "crates/ralphy-daemon/assets/ui",
            "crates/ralphy-daemon/ui-tests",
        ],
    ),
    // A folder, not a list of files: on Windows `npx` runs through
    // `cmd.exe`, which cuts a command line at 8191 characters.
    step(
        "oxlint-types",
        &["UI type-aware lint (oxlint with tsgolint)"],
        "npx",
        &[
            "-y",
            "-p",
            "oxlint@1.93.0",
            "-p",
            "oxlint-tsgolint@7.0.2006",
            "oxlint",
            "--type-aware",
            "--deny-warnings",
            "crates/ralphy-daemon/assets/ui",
        ],
    ),
    // A pattern that oxfmt expands, for the same reason.
    step(
        "oxfmt",
        &["UI format (oxfmt)"],
        "npx",
        &[
            "-y",
            "oxfmt@0.75.0",
            "--check",
            "crates/ralphy-daemon/assets/ui/*.ts",
        ],
    ),
    step(
        "jscpd",
        &["No new clone against the base branch"],
        "npx",
        &[
            "-y",
            "jscpd@5.4.0",
            "--baseline-from-ref",
            "main",
            "--fail-on-new-clones",
            "0",
            "crates",
        ],
    ),
    Step {
        needs: Some("cargo-nextest"),
        ..step(
            "ratchets",
            &["Ratchets (xtask tests)"],
            "cargo",
            &["nextest", "run", "--locked", "-p", "xtask"],
        )
    },
    Step {
        needs: Some("cargo-nextest"),
        slow: true,
        ..step(
            "nextest",
            &["Test"],
            "cargo",
            &[
                "nextest",
                "run",
                "--locked",
                "--workspace",
                "--exclude",
                "xtask",
                "--no-fail-fast",
            ],
        )
    },
    Step {
        slow: true,
        ..step(
            "doctest",
            &["Doctests"],
            "cargo",
            &["test", "--locked", "--workspace", "--doc"],
        )
    },
];
