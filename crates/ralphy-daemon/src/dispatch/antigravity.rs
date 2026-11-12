//! The argv the daemon runs before an Antigravity console when the owned root
//! has no hooks file (ADR-0086 D8, ADR-0040 Amendment 3).

/// `antigravity prepare-root`: fixed, with no client input. The root is one per
/// OS user, so the command needs no repository argument.
pub fn antigravity_root_argv() -> Vec<String> {
    vec!["antigravity".to_owned(), "prepare-root".to_owned()]
}
