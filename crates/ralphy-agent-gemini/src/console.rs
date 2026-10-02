//! The owned root for a workbench console (ADR-0040 Amendment 3).
//!
//! The daemon starts a Gemini console itself, without the adapter, so it cannot
//! write the owned root and the policy document (ADR-0032 §10). It runs
//! `ralphy gemini prepare-root`, which lands here and pays the SAME
//! [`crate::prepare_root`] a run and every one-shot pay. There is one
//! generator, so the console's policy cannot drift from a run's.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// Prepare the owned root and the policy document under `<repo_root>/.ralphy`,
/// and return the policy document's path.
///
/// No child is started: the skill-discovery receipt is a run's advisory extra,
/// and a console acts on nothing it reports. An administrator control that
/// disables autonomy is an error here, as it is for a run (D5).
pub fn prepare_console_root(repo_root: &Path) -> Result<PathBuf> {
    Ok(crate::prepare_root(&repo_root.join(".ralphy"))?.policy_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The daemon's gate keys on this exact file, so the console path must
    /// produce it — and a policy that still denies delegation, not an empty one.
    #[test]
    fn writes_the_policy_document_the_daemon_gates_on() {
        let repo = tempfile::tempdir().unwrap();
        let policy = prepare_console_root(repo.path()).unwrap();
        assert_eq!(
            policy,
            repo.path()
                .join(".ralphy")
                .join("gemini-home")
                .join(".gemini")
                .join("ralphy-policy.toml")
        );
        let body = std::fs::read_to_string(&policy).unwrap();
        assert!(body.contains("invoke_agent"), "{body}");
    }
}
