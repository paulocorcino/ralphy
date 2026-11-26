use super::*;

/// A repo whose `.ralphy/settings.json` holds `body`.
fn repo_with(d: &tempfile::TempDir, body: &str) -> PathBuf {
    let repo = d.path().to_path_buf();
    std::fs::create_dir_all(repo.join(".ralphy")).unwrap();
    std::fs::write(repo.join(".ralphy").join("settings.json"), body).unwrap();
    repo
}

fn mark(spec: &SessionSpec) -> Vec<&OsString> {
    spec.env
        .iter()
        .filter(|(k, _)| k == ralphy_proc_util::GUARD_CONSOLE_ENV)
        .map(|(_, v)| v)
        .collect()
}

const ON: &str = r#"{"antigravity":{"console_agent_workflow_i_understand_the_risk":true}}"#;

/// On, an Antigravity console carries the mark at `1`; off, or for any other
/// vendor, no console carries it.
#[test]
fn only_an_opted_in_antigravity_console_is_marked() {
    let d = tempfile::tempdir().unwrap();
    let on = repo_with(&d, ON);
    let spec = spec_for(Agent::Antigravity, &on, on.clone(), "owner/r", (24, 80));
    if spec.program.is_empty() {
        return; // no owned root on this machine: the console is refused anyway
    }
    assert_eq!(mark(&spec), vec![&OsString::from("1")]);
    for agent in Agent::ALL.iter().filter(|a| **a != Agent::Antigravity) {
        let other = spec_for(*agent, &on, on.clone(), "owner/r", (24, 80));
        assert!(mark(&other).is_empty(), "{agent:?} must not get it");
    }
    for body in [
        "",
        "{",
        r#"{"antigravity":{"console_agent_workflow_i_understand_the_risk":"true"}}"#,
        r#"{"claude":{"console_agent_workflow_i_understand_the_risk":true}}"#,
    ] {
        let off = repo_with(&d, body);
        let spec = spec_for(Agent::Antigravity, &off, off.clone(), "owner/r", (24, 80));
        assert!(mark(&spec).is_empty(), "settings.json = {body}");
    }
}

/// The daemon reparses the adapter's schema; a rename there would read
/// `false` for every repo that opted in.
#[test]
fn the_console_workflow_key_matches_the_adapters_own_schema() {
    let src = include_str!("../../../../../ralphy-agent-antigravity/src/settings.rs");
    assert!(
        src.contains("pub console_agent_workflow_i_understand_the_risk: bool,"),
        "the adapter renamed or retyped the opt-in the daemon reparses"
    );
    assert!(src.contains(r#"SECTION: &'static str = "antigravity""#));
}

const NOTICE_TEXT: &str = concat!(
    "The agent may switch branches, push and write pull requests ",
    "in this console."
);

/// The notice is the sentence as dim text on a line of its own.
fn dim_line(text: &str) -> String {
    format!("\u{1b}[2m{text}\u{1b}[0m\r\n")
}

#[test]
fn a_console_with_the_setting_on_carries_the_notice() {
    let d = tempfile::tempdir().unwrap();
    let on = repo_with(&d, ON);
    let spec = spec_for(Agent::Antigravity, &on, on.clone(), "owner/r", (24, 80));
    if spec.program.is_empty() {
        return; // no owned root on this machine: the console is refused anyway
    }
    assert_eq!(spec.start_notice, Some(dim_line(NOTICE_TEXT)));
}

#[test]
fn a_console_with_the_setting_off_carries_no_notice() {
    let d = tempfile::tempdir().unwrap();
    for body in [
        "{}",
        r#"{"antigravity":{"console_agent_workflow_i_understand_the_risk":false}}"#,
        r#"{"antigravity":{"console_agent_workflow_i_understand_the_risk":"true"}}"#,
    ] {
        let off = repo_with(&d, body);
        let spec = spec_for(Agent::Antigravity, &off, off.clone(), "owner/r", (24, 80));
        assert_eq!(spec.start_notice, None, "settings.json = {body}");
    }
}

#[test]
fn a_claude_console_carries_no_notice() {
    let d = tempfile::tempdir().unwrap();
    let on = repo_with(&d, ON);
    let spec = spec_for(Agent::Claude, &on, on.clone(), "owner/r", (24, 80));
    assert_eq!(spec.start_notice, None);
}
