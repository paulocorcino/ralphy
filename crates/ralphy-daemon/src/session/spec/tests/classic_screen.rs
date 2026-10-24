use super::*;

/// A repo whose `.ralphy/settings.json` holds `body`.
fn repo_with(d: &tempfile::TempDir, body: &str) -> PathBuf {
    let repo = d.path().to_path_buf();
    std::fs::create_dir_all(repo.join(".ralphy")).unwrap();
    std::fs::write(repo.join(".ralphy").join("settings.json"), body).unwrap();
    repo
}

fn classic_env(spec: &SessionSpec) -> Vec<&OsString> {
    spec.env
        .iter()
        .filter(|(k, _)| k == "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN")
        .map(|(_, v)| v)
        .collect()
}

/// Every way of failing to read the opt-in leaves Claude's own renderer
/// choice alone: the variable overrides the operator's Claude settings, so
/// only an explicit `true` may set it.
#[test]
fn an_unreadable_settings_file_never_sets_the_classic_screen() {
    let d = tempfile::tempdir().unwrap();
    assert!(
        !claude_console_classic_screen(d.path()),
        "no .ralphy at all"
    );
    for body in [
        "",
        "{",
        r#"{"claude":{}}"#,
        r#"{"claude":{"console_classic_screen":"true"}}"#,
        r#"{"claude":{"console_classic_screen":false}}"#,
        r#"{"claude":{"console_name":true}}"#,
        r#"{"cursor":{"console_classic_screen":true}}"#,
    ] {
        let repo = repo_with(&d, body);
        assert!(
            !claude_console_classic_screen(&repo),
            "settings.json = {body} must not set the classic screen"
        );
    }
    let repo = repo_with(&d, r#"{"claude":{"console_classic_screen":true}}"#);
    assert!(
        claude_console_classic_screen(&repo),
        "the one shape that opts in"
    );
}

/// On, a Claude console starts with the variable at `1`; off, it carries
/// none, and no other vendor is ever handed it.
#[test]
fn a_claude_console_gets_the_classic_screen_only_when_opted_in() {
    let d = tempfile::tempdir().unwrap();
    let on = repo_with(&d, r#"{"claude":{"console_classic_screen":true}}"#);
    let spec = spec_for(Agent::Claude, &on, on.clone(), "owner/ralphy", 24, 80);
    assert_eq!(classic_env(&spec), vec![&OsString::from("1")]);
    assert!(
        spec.name.is_none(),
        "the classic screen does not name the console"
    );
    for agent in Agent::ALL.iter().filter(|a| **a != Agent::Claude) {
        let other = spec_for(*agent, &on, on.clone(), "owner/ralphy", 24, 80);
        assert!(classic_env(&other).is_empty(), "{agent:?} must not get it");
    }

    let off = repo_with(&d, r#"{"claude":{"console_name":true}}"#);
    let spec = spec_for(Agent::Claude, &off, off.clone(), "owner/ralphy", 24, 80);
    assert!(classic_env(&spec).is_empty(), "off by default");
}

/// The daemon reparses the adapter's schema; a rename or retype there would
/// read `false` for every repo that opted in.
#[test]
fn the_classic_screen_key_matches_the_adapters_own_schema() {
    let src = include_str!("../../../../../ralphy-agent-claude/src/settings.rs");
    assert!(
        src.contains("pub console_classic_screen: bool,"),
        "the adapter renamed or retyped the opt-in the daemon reparses"
    );
}
