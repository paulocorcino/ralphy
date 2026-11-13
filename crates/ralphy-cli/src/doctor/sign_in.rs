//! The Antigravity sign-in line: whether `agy` is signed in for Ralphy's root.

use super::{MachineFacts, Tool, ToolLine};

/// The line for an installed `agy`; `None` when `agy` is not installed. A
/// signed-out `agy` is a warning, never a missing tool.
pub(super) fn sign_in_line(facts: &MachineFacts) -> Option<ToolLine> {
    let signed_in = facts.antigravity_signed_in?;
    let (text, fix) = if signed_in {
        ("signed in for Ralphy", None)
    } else {
        (
            "not signed in for Ralphy, or agy models failed",
            sign_in_command(),
        )
    };
    Some(ToolLine {
        tool: Tool::SignIn,
        name: "antigravity sign-in".to_owned(),
        ok: signed_in,
        required: false,
        version: None,
        text: text.to_owned(),
        fix,
    })
}

/// The command that starts `agy` in Ralphy's root, where the operator signs in.
fn sign_in_command() -> Option<String> {
    match ralphy_proc_util::antigravity::owned_root() {
        Ok(root) => Some(format!("agy --gemini_dir=\"{}\"", root.display())),
        Err(e) => {
            tracing::debug!(error = %format!("{e:#}"), "no Antigravity root for the sign-in fix");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::{evaluate, MachineFacts};
    use crate::init::Agent;

    fn facts(signed_in: Option<bool>) -> MachineFacts {
        MachineFacts {
            antigravity_signed_in: signed_in,
            ..MachineFacts::default()
        }
    }

    #[test]
    fn the_agy_sign_in_line_reads_signed_in() {
        let line = sign_in_line(&facts(Some(true))).expect("a line");
        assert!(line.ok);
        assert!(!line.required);
        assert_eq!(line.text, "signed in for Ralphy");
        assert_eq!(line.name, "antigravity sign-in");
    }

    #[test]
    fn the_agy_sign_in_line_warns_with_the_sign_in_command() {
        let line = sign_in_line(&facts(Some(false))).expect("a line");
        assert!(!line.ok);
        assert!(!line.required);
        let fix = line.fix.expect("a fix");
        assert!(fix.starts_with("agy --gemini_dir="), "{fix}");
    }

    #[test]
    fn no_agy_sign_in_line_without_agy() {
        assert_eq!(sign_in_line(&facts(None)), None);
        let report = evaluate(&facts(None), false);
        assert!(!report.tools.iter().any(|t| t.tool == Tool::SignIn));
    }

    #[test]
    fn a_signed_out_agy_is_not_a_missing_tool() {
        let all = MachineFacts {
            git: Some("git version 2".to_owned()),
            gh: Some("gh version 2".to_owned()),
            gh_signed_in: true,
            python: Some("Python 3".to_owned()),
            agents: vec![(Agent::Antigravity, "agy 1".to_owned())],
            antigravity_signed_in: Some(false),
            ..MachineFacts::default()
        };
        let report = evaluate(&all, false);
        let line = report
            .tools
            .iter()
            .find(|t| t.tool == Tool::SignIn)
            .expect("the sign-in line");
        assert!(!line.ok);
        assert_eq!(report.missing_required(), 0);
    }
}
