use super::*;

#[test]
fn no_shell_and_git_bash_found_pins_it() {
    let bash = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
    assert_eq!(git_bash_shell_pin(None, Some(bash.clone())), Some(bash));
}

#[test]
fn an_operator_set_shell_is_never_overridden() {
    let bash = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
    assert_eq!(
        git_bash_shell_pin(Some("/usr/bin/fish".into()), Some(bash)),
        None,
        "a deliberate operator SHELL must pass through untouched (D8 stance)"
    );
}

#[test]
fn no_git_bash_located_sets_nothing() {
    assert_eq!(
        git_bash_shell_pin(None, None),
        None,
        "SHELL must never point at a missing binary — PowerShell fallback stands"
    );
}

/// The builder actually applies the pin: whatever the pure decision says for
/// this host's real env, the `Command` reflects it. Recomputing the same
/// inputs keeps it deterministic while still catching a wrong var name or a
/// missing gate.
#[test]
fn build_cursor_command_applies_the_pin() {
    let cmd = build_cursor_command("s1", None, Path::new("."), Path::new("cfg"));
    let expected = git_bash_shell_pin(
        std::env::var_os("SHELL"),
        ralphy_proc_util::locate_git_bash(),
    );
    match expected {
        Some(p) => {
            assert_eq!(env_of(&cmd, "SHELL").map(PathBuf::from), Some(p));
            // MSYSTEM must ride along, or git-bash returns "no exit status".
            assert_eq!(
                env_of(&cmd, "MSYSTEM").as_deref(),
                Some("MINGW64"),
                "pinning SHELL to git-bash without MSYSTEM breaks the shell"
            );
        }
        None => {
            assert!(
                !cmd.get_envs().any(|(k, _)| k == "SHELL"),
                "SHELL must not be set when the pin declines"
            );
            assert!(
                !cmd.get_envs().any(|(k, _)| k == "MSYSTEM"),
                "MSYSTEM must not be set when SHELL is not pinned"
            );
        }
    }
    // The one-shot builder delegates, so it inherits the same wiring.
    let init = build_cursor_init_command(None, Path::new("."), Path::new("cfg"));
    assert_eq!(env_of(&init, "SHELL"), env_of(&cmd, "SHELL"));
    assert_eq!(env_of(&init, "MSYSTEM"), env_of(&cmd, "MSYSTEM"));
}
