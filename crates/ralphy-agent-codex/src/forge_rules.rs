//! The forge writes a headless Codex session must not run (ADR-0072 D6, ADR-0004
//! amendment of 2026-10-01): a `forbidden` exec-policy rule file in the session's
//! own `.codex/rules/`, kept out of git by a merged `.codex/rules/.gitignore`.
//!
//! Codex loads rules only from `$CODEX_HOME/rules` or a project's `.codex/rules`,
//! and has no `-c` key for a rules path (codex-cli 0.159.2). Measured with
//! 0.159.2 under `-s danger-full-access`, in a live run (FinCal, 2026-10-01): the
//! project rules load, a forbidden prefix is refused before the process starts,
//! and the model reads the justification.

use std::ffi::OsString;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use ralphy_adapter_support::{ensure_gitignore_entries, DENIED_FORGE_WRITES};

/// The rule file's name inside `.codex/rules/`.
const RULES_FILE: &str = "ralphy-forge.rules";

/// The text the model reads when a rule refuses a command.
const JUSTIFICATION: &str = "Ralphy: the operator pushes and writes pull requests, not the agent";

/// One `prefix_rule` per denied command. A prefix rule matches the start of the
/// argv, so every argument after the listed words is covered.
fn forge_rules() -> String {
    DENIED_FORGE_WRITES
        .iter()
        .map(|words| {
            let pattern = words
                .iter()
                .map(|w| format!("\"{w}\""))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "prefix_rule(pattern=[{pattern}], decision=\"forbidden\", justification=\"{JUSTIFICATION}\")\n"
            )
        })
        .collect()
}

/// Write the rule file into `<dir>/.codex/rules/` and list it in that folder's
/// `.gitignore`, so the dirty-tree check never sees it. The file is rewritten on
/// every call, so a changed list takes effect on the next session.
pub(crate) fn install_forge_rules(dir: &Path) -> Result<()> {
    let rules_dir = dir.join(".codex").join("rules");
    fs::create_dir_all(&rules_dir).with_context(|| format!("creating {}", rules_dir.display()))?;
    let path = rules_dir.join(RULES_FILE);
    fs::write(&path, forge_rules()).with_context(|| format!("writing {}", path.display()))?;
    ensure_gitignore_entries(&rules_dir.join(".gitignore"), &[OsString::from(RULES_FILE)])
        .with_context(|| format!("hiding {} from git", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn forge_rules_forbid_each_denied_write_and_nothing_else() {
        let text = forge_rules();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), DENIED_FORGE_WRITES.len(), "{text}");
        for (line, words) in lines.iter().zip(DENIED_FORGE_WRITES) {
            let pattern = words
                .iter()
                .map(|w| format!("\"{w}\""))
                .collect::<Vec<_>>()
                .join(", ");
            assert!(
                line.starts_with(&format!("prefix_rule(pattern=[{pattern}],")),
                "{line}"
            );
            assert!(line.contains("decision=\"forbidden\""), "{line}");
        }
        for allowed in ["\"view\"", "\"list\"", "\"api\"", "\"status\""] {
            assert!(
                !text.contains(allowed),
                "{allowed} must stay allowed: {text}"
            );
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn the_rule_file_is_written_and_hidden_from_git() {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        for _ in 0..2 {
            install_forge_rules(dir.path()).expect("install");
        }
        let rules = dir.path().join(".codex/rules");
        assert_eq!(
            fs::read_to_string(rules.join(RULES_FILE)).unwrap(),
            forge_rules()
        );
        assert_eq!(git(dir.path(), &["status", "--porcelain", "-uall"]), "");
    }
}
