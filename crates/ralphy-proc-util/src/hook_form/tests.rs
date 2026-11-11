use super::*;

const PROGRAM: &str = if cfg!(windows) {
    r"C:\tools\ralphy.exe"
} else {
    "/tools/ralphy"
};

fn hook(version: Option<&str>) -> HookCommand {
    hook_for_version(version, Path::new(PROGRAM), &["hook", "status"])
}

#[test]
fn the_form_follows_the_claude_code_version() {
    let rows = [
        (
            Some("2.1.138 (Claude Code)"),
            HookForm::Shell,
            "2.1.138 is older",
        ),
        (
            Some("2.1.139 (Claude Code)"),
            HookForm::Exec,
            "2.1.139 runs",
        ),
        (
            Some("2.1.312 (Claude Code)"),
            HookForm::Exec,
            "2.1.312 runs",
        ),
        (
            Some("not a version"),
            HookForm::Shell,
            "not a version number",
        ),
        (None, HookForm::Shell, "could not be read"),
    ];
    for (version, form, reason) in rows {
        let got = hook(version);
        assert_eq!(got.form(), form, "{version:?}");
        assert!(
            got.reason().contains(reason),
            "{version:?}: {}",
            got.reason()
        );
    }
}

#[test]
fn an_exec_entry_names_the_program_and_its_args() {
    let entry = hook(Some("2.1.200 (Claude Code)")).entry();
    assert_eq!(
        entry,
        json!({"type": "command", "command": PROGRAM, "args": ["hook", "status"]})
    );
    let command = entry["command"].as_str().unwrap_or("");
    assert!(!command.contains(['\'', '"']), "{command}");
}

#[test]
fn a_shell_command_quotes_every_shell_character() {
    let path = Path::new("/a b/$x/`y`/it's/\"q\"/ralphy");
    let entry = hook_for_version(None, path, &["hook", "status"]).entry();
    assert_eq!(
        entry["command"],
        "'/a b/$x/`y`/it'\\''s/\"q\"/ralphy' hook status"
    );
}

#[test]
fn no_form_carries_the_verbatim_prefix() {
    let program = Path::new(r"\\?\C:\tools\ralphy.exe");
    for version in [Some("2.1.200"), Some("2.1.1"), None] {
        let entry = hook_for_version(version, program, &["hook", "status"]).entry();
        let command = entry["command"].as_str().unwrap_or("");
        if cfg!(windows) {
            assert!(!command.contains(r"\\?\"), "{command}");
        }
        if cfg!(windows) && entry.get("args").is_none() {
            assert!(!command.contains('\\'), "{command}");
        }
    }
}

#[test]
fn a_shell_form_reason_is_logged_once_per_binary() {
    let dir = tempfile::tempdir().expect("temp dir");
    let bin = dir.path().join("claude");
    let reason = hook(Some("2.1.138 (Claude Code)")).reason().to_owned();
    assert!(first_warning(&bin, &reason), "the first hook logs");
    assert!(!first_warning(&bin, &reason), "the second hook does not");
    let other = dir.path().join("other-claude");
    assert!(first_warning(&other, &reason), "another binary logs");
    assert!(first_warning(&bin, "another reason"), "another reason logs");
}

#[test]
fn disable_all_hooks_names_the_file_that_sets_it() {
    let home = tempfile::tempdir().expect("temp dir");
    let project = tempfile::tempdir().expect("temp dir");
    let write = |root: &Path, body: &str| {
        let dir = root.join(".claude");
        std::fs::create_dir_all(&dir).expect("create dir");
        std::fs::write(dir.join("settings.json"), body).expect("write file");
    };
    let user = home.path().join(".claude").join("settings.json");
    let proj = project.path().join(".claude").join("settings.json");
    let found = || disable_all_hooks(Some(home.path()), Some(project.path()));
    assert_eq!(found(), None, "missing files");
    write(home.path(), r#"{"disableAllHooks": true}"#);
    assert_eq!(found(), Some(user));
    write(home.path(), r#"{"disableAllHooks": false}"#);
    assert_eq!(found(), None, "false");
    write(home.path(), "{}");
    assert_eq!(found(), None, "missing key");
    write(home.path(), "{ not json");
    assert_eq!(found(), None, "bad json");
    write(project.path(), r#"{"disableAllHooks": true}"#);
    assert_eq!(found(), Some(proj));
}

#[test]
fn the_named_file_uses_one_kind_of_slash() {
    let project = tempfile::tempdir().expect("temp dir");
    let dir = project.path().join(".claude");
    std::fs::create_dir_all(&dir).expect("create dir");
    std::fs::write(
        dir.join("settings.local.json"),
        r#"{"disableAllHooks": true}"#,
    )
    .expect("write file");
    // The root as `git rev-parse --show-toplevel` prints it on Windows.
    let root = PathBuf::from(project.path().to_string_lossy().replace('\\', "/"));
    let found = disable_all_hooks(None, Some(&root)).expect("the local file");
    let text = found.display().to_string();
    let other = if cfg!(windows) { '/' } else { '\\' };
    assert!(!text.contains(other), "{text}");
    assert!(text.ends_with(&format!(
        ".claude{0}settings.local.json",
        std::path::MAIN_SEPARATOR
    )));
}

/// Codex on Windows runs a hook through PowerShell, where a quoted path
/// is a string and not a call: the command starts with the call operator.
#[test]
fn codex_hook_uses_the_call_operator_on_windows() {
    let exe = Path::new(r"C:\Users\a b\ralphy.exe");
    let windows = codex_hook_command_for(exe, true);
    assert_eq!(windows, r#"& "C:/Users/a b/ralphy.exe" hook status codex"#);
    assert!(windows.starts_with("& \"") && windows.ends_with("hook status codex"));
    let unix = codex_hook_command_for(Path::new("/a b/ralphy"), false);
    assert_eq!(unix, "'/a b/ralphy' hook status codex");
    // Each `-c` value is TOML: the quotes of the command are escaped.
    let args = codex_hook_args(exe, &["Stop"]);
    let want = if cfg!(windows) {
        concat!(
            r#"hooks.Stop=[{hooks=[{type="command","#,
            r#"command="& \"C:/Users/a b/ralphy.exe\" hook status codex"}]}]"#
        )
    } else {
        concat!(
            r#"hooks.Stop=[{hooks=[{type="command","#,
            r#"command="'C:\\Users\\a b\\ralphy.exe' hook status codex"}]}]"#
        )
    };
    assert_eq!(args, [CODEX_HOOK_TRUST_FLAG, "-c", want]);
}

/// A project's own Codex hooks are a `hooks.json` or a `config.toml`
/// with a root `hooks` key; a config that does not parse counts as
/// hooks, and so does one in the repo root above the folder.
#[test]
fn a_project_with_its_own_codex_hooks_is_seen() {
    // (why, the file under `.codex` and its text, has hooks); `None` is
    // an empty `.codex`, and an empty name is no `.codex` at all.
    type Row<'a> = (&'a str, Option<(&'a str, &'a str)>, bool);
    let rows: [Row; 6] = [
        ("hooks.json", Some(("hooks.json", "{}")), true),
        (
            "config with hooks",
            Some(("config.toml", "[[hooks.Stop]]\nhooks = []\n")),
            true,
        ),
        (
            "config without hooks",
            Some(("config.toml", "model = \"m\"\n[tui]\nhooks = 1\n")),
            false,
        ),
        ("broken config", Some(("config.toml", "hooks = = [")), true),
        ("an empty .codex", None, false),
        ("no .codex", Some(("", "")), false),
    ];
    for (why, file, want) in rows {
        let dir = tempfile::tempdir().expect("a temp dir");
        match file {
            Some(("", _)) => {}
            Some((name, text)) => {
                std::fs::create_dir_all(dir.path().join(".codex")).expect("the .codex dir");
                std::fs::write(dir.path().join(".codex").join(name), text).expect("the file");
            }
            None => std::fs::create_dir_all(dir.path().join(".codex")).expect("the .codex dir"),
        }
        assert_eq!(codex_project_has_hooks(dir.path()), want, "{why}");
    }
    let repo = tempfile::tempdir().expect("a temp dir");
    std::fs::create_dir_all(repo.path().join(".git")).expect("the .git dir");
    std::fs::create_dir_all(repo.path().join(".codex")).expect("the .codex dir");
    std::fs::write(repo.path().join(".codex/hooks.json"), "{}").expect("the file");
    let sub = repo.path().join("crates").join("a");
    std::fs::create_dir_all(&sub).expect("the sub dir");
    assert!(codex_project_has_hooks(&sub), "the repo root's hooks");
}
