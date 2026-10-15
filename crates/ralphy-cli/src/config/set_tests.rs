use super::*;

fn workspace() -> (Workspace, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a temp dir");
    (Workspace::new(dir.path()), dir)
}

/// A blank value is refused before anything is written, with the hint to
/// use `config unset` instead.
#[test]
fn set_refuses_an_empty_value() {
    let (ws, _dir) = workspace();
    let err = set(&ws, "verify.command", "   ").unwrap_err();
    assert_eq!(
        err.to_string(),
        "value for 'verify.command' must not be empty — use `config unset verify.command` to clear it"
    );
    assert!(!ws.settings_path().exists(), "nothing is written");
}

/// `verify.require_verify_gate` takes only `true` or `false`.
#[test]
fn set_refuses_a_verify_gate_that_is_not_a_bool() {
    let (ws, _dir) = workspace();
    let err = set(&ws, "verify.require_verify_gate", "maybe").unwrap_err();
    assert_eq!(
        err.to_string(),
        "verify.require_verify_gate must be 'true' or 'false', got 'maybe'"
    );
    assert_eq!(
        Settings::load(&ws).unwrap().verify.require_verify_gate,
        None
    );
}

/// `claude.console_name` takes only `true` or `false`.
#[test]
fn set_refuses_a_console_name_that_is_not_a_bool() {
    let (ws, _dir) = workspace();
    let err = set(&ws, "claude.console_name", "yes").unwrap_err();
    assert_eq!(
        err.to_string(),
        "claude.console_name must be 'true' or 'false', got 'yes'"
    );
    let claude: ClaudeSettings = Settings::load(&ws)
        .unwrap()
        .agent_settings(ClaudeSettings::SECTION)
        .unwrap();
    assert!(!claude.console_name);
}
