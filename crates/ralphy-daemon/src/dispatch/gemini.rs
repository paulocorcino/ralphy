//! The argv the daemon runs before a Gemini console in a repo with no owned
//! configuration root (ADR-0040 Amendment 3).

/// `gemini prepare-root`: fixed, with no client input. Run in the repo's
/// primary tree, where the console's policy document is looked for.
pub fn gemini_root_argv() -> Vec<String> {
    ["gemini", "prepare-root"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}
