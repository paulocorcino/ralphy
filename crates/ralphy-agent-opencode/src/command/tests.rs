use super::*;
use crate::cleanup::remove_scratch;

fn squeezed(source: &str) -> String {
    source.split_whitespace().collect()
}

#[test]
fn the_program_comes_from_the_shared_opencode_locator() {
    let command = squeezed(include_str!("../command.rs"));
    assert!(
        command.contains("ralphy_proc_util::opencode::locate_opencode()"),
        "the locator must come from proc-util"
    );
    assert!(
        command.contains("Command::new(resolve_opencode_program())"),
        "the run command must spawn the located program"
    );
    let tasks = squeezed(include_str!("../tasks.rs"));
    assert!(
        tasks.contains("Command::new(resolve_opencode_program())"),
        "the one-shot task must spawn the located program"
    );
}

fn argv(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn the_run_command_sets_pwd_to_its_folder() {
    let root = Path::new("/repo/work");
    let state = StatePlugin {
        plugin: Path::new("/run/plugin.js"),
        status: Path::new("/run/status"),
    };
    for cmd in [
        build_opencode_command(None, None, root, None),
        build_watched_opencode_command(None, None, root, None, &state),
    ] {
        let pwd = cmd.get_envs().find(|(k, _)| *k == "PWD");
        assert_eq!(
            pwd,
            Some((std::ffi::OsStr::new("PWD"), Some(root.as_os_str())))
        );
    }
}

#[test]
fn build_command_omits_model_when_none() {
    let cmd = build_opencode_command(None, None, Path::new("/repo"), None);
    // The program is `resolve_opencode_program()`: a full path (e.g.
    // `opencode.cmd` on Windows) when found on PATH, else the bare name. Either
    // way the file stem is `opencode`.
    let program = PathBuf::from(cmd.get_program());
    assert_eq!(
        program.file_stem().and_then(|s| s.to_str()),
        Some("opencode"),
        "program: {program:?}"
    );
    let args = argv(&cmd);
    assert!(args.contains(&"run".to_owned()), "argv: {args:?}");
    // No -m flag is passed; OpenCode resolves its own model (ADR-0005 D4).
    assert!(!args.contains(&"-m".to_owned()), "argv: {args:?}");
    // --variant is absent unless the operator set one (D3).
    assert!(!args.contains(&"--variant".to_owned()), "argv: {args:?}");
    // Always-on flags.
    assert!(
        args.contains(&"--dangerously-skip-permissions".to_owned()),
        "argv: {args:?}"
    );
    assert!(args.contains(&"--format".to_owned()), "argv: {args:?}");
    assert!(args.contains(&"json".to_owned()), "argv: {args:?}");
    // `--print-logs`/`--log-level ERROR` route opencode's own logs to stderr so
    // a provider quota block that never reaches the JSON stream is still visible
    // to the limit detector (D9 — silent-quota fix, FinCal #71).
    assert!(args.contains(&"--print-logs".to_owned()), "argv: {args:?}");
    assert!(args.contains(&"--log-level".to_owned()), "argv: {args:?}");
    assert!(args.contains(&"ERROR".to_owned()), "argv: {args:?}");
}

#[test]
fn build_command_includes_model_when_some() {
    let cmd = build_opencode_command(
        Some("anthropic/claude-sonnet-4-6"),
        None,
        Path::new("/repo"),
        None,
    );
    let args = argv(&cmd);
    assert!(args.contains(&"-m".to_owned()), "argv: {args:?}");
    assert!(
        args.contains(&"anthropic/claude-sonnet-4-6".to_owned()),
        "argv: {args:?}"
    );
}

#[test]
fn build_command_includes_variant_only_when_some() {
    let without = build_opencode_command(None, None, Path::new("/repo"), None);
    assert!(!argv(&without).contains(&"--variant".to_owned()));

    let with = build_opencode_command(None, Some("high"), Path::new("/repo"), None);
    let args = argv(&with);
    assert!(args.contains(&"--variant".to_owned()), "argv: {args:?}");
    assert!(args.contains(&"high".to_owned()), "argv: {args:?}");
}

#[test]
fn build_command_removes_both_api_keys() {
    let cmd = build_opencode_command(None, None, Path::new("/repo"), None);
    let anthropic_removed = cmd
        .get_envs()
        .any(|(k, v)| k == "ANTHROPIC_API_KEY" && v.is_none());
    let openai_removed = cmd
        .get_envs()
        .any(|(k, v)| k == "OPENAI_API_KEY" && v.is_none());
    assert!(
        anthropic_removed,
        "ANTHROPIC_API_KEY should be removed on the child"
    );
    assert!(
        openai_removed,
        "OPENAI_API_KEY should be removed on the child"
    );
}

fn injected_config(cmd: &Command) -> serde_json::Value {
    let raw = cmd
        .get_envs()
        .find(|(k, _)| *k == "OPENCODE_CONFIG_CONTENT")
        .and_then(|(_, v)| v)
        .map(|v| v.to_string_lossy().into_owned())
        .expect("OPENCODE_CONFIG_CONTENT is set");
    serde_json::from_str(&raw).expect("the injected config is JSON")
}

#[test]
fn build_command_injects_the_skills_path_only_when_given() {
    let dir = std::env::temp_dir().join("ralphy-opencode-skills-cfg");
    std::fs::create_dir_all(&dir).unwrap();
    let cmd = build_opencode_command(None, None, Path::new("/repo"), Some(&dir));
    let expected = dir.canonicalize().unwrap_or_else(|_| dir.clone());
    assert_eq!(
        injected_config(&cmd)["skills"]["paths"],
        serde_json::json!([expected])
    );

    let cfg = injected_config(&build_opencode_command(
        None,
        None,
        Path::new("/repo"),
        None,
    ));
    assert!(cfg.get("skills").is_none(), "{cfg}");
    remove_scratch(&dir);
}

/// A plan or an execute names the state plugin in the run dir under
/// `plugin`, keeps the forge-write denies, and gives the plugin the
/// status file; a task call names no plugin.
#[test]
fn opencode_config_names_the_state_plugin() {
    let run_dir = if cfg!(windows) {
        PathBuf::from(r"C:\runs\r 1")
    } else {
        PathBuf::from("/runs/r 1")
    };
    let plugin = run_dir.join("ralphy-state-plugin.js");
    let status = run_dir.join("agent-status.jsonl");
    let state = StatePlugin {
        plugin: &plugin,
        status: &status,
    };
    let cmd = build_watched_opencode_command(None, None, Path::new("/repo"), None, &state);
    let cfg = injected_config(&cmd);
    let want = if cfg!(windows) {
        "file:///C:/runs/r 1/ralphy-state-plugin.js"
    } else {
        "file:///runs/r 1/ralphy-state-plugin.js"
    };
    assert_eq!(cfg["plugin"], serde_json::json!([want]), "{cfg}");
    let rules = cfg["permission"]["bash"].as_object().expect("the deny map");
    assert_eq!(rules.get("git push"), Some(&"deny".into()), "{cfg}");
    assert_eq!(rules.get("gh pr merge *"), Some(&"deny".into()), "{cfg}");
    let env = cmd
        .get_envs()
        .find(|(k, _)| *k == "RALPHY_STATUS_FILE")
        .and_then(|(_, v)| v)
        .map(PathBuf::from);
    assert_eq!(env, Some(status));

    let task = build_opencode_command(None, None, Path::new("/repo"), None);
    assert!(injected_config(&task).get("plugin").is_none());
    assert!(!task.get_envs().any(|(k, _)| k == "RALPHY_STATUS_FILE"));
}

/// A daemon or a run started through a canonicalized path has the
/// verbatim form; the entry drops it.
#[cfg(windows)]
#[test]
fn the_plugin_entry_drops_the_verbatim_prefix() {
    assert_eq!(
        plugin_entry(Path::new(r"\\?\C:\r\p.js")),
        "file:///C:/r/p.js"
    );
}

/// opencode's documented wildcard rule: `*` is any run of characters and
/// the pattern must match the whole command.
fn glob(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((head, tail)) => {
            let Some(rest) = text.strip_prefix(head) else {
                return false;
            };
            rest.char_indices()
                .map(|(i, _)| i)
                .chain([rest.len()])
                .any(|i| glob(tail, rest.split_at(i).1))
        }
    }
}

/// Every session, with or without skills, carries the deny map, and the
/// map denies the forge writes and nothing else.
#[test]
fn every_session_denies_the_forge_writes_and_only_them() {
    let skills = std::env::temp_dir();
    for skills_dir in [None, Some(skills.as_path())] {
        let cmd = build_opencode_command(None, None, Path::new("/repo"), skills_dir);
        let cfg = injected_config(&cmd);
        let rules = cfg["permission"]["bash"]
            .as_object()
            .expect("permission.bash is a map");
        assert!(rules.values().all(|v| v == "deny"), "{cfg}");
        assert!(!rules.contains_key("*"), "the operator's own rules apply");
        let denied = |command: &str| rules.keys().any(|p| glob(p, command));
        for command in [
            "git push",
            "git push origin HEAD",
            "gh pr create --fill",
            "gh pr edit 5",
            "gh pr ready",
            "gh pr reopen 5",
            "gh pr review 5 --approve",
            "gh pr comment 5 -b hi",
            "gh pr merge 5",
            "gh pr close 5",
        ] {
            assert!(denied(command), "{command} must be denied");
        }
        for command in [
            "git status",
            "git commit -m \"fix push logic\"",
            "git pull",
            "gh pr view 5",
            "gh pr list",
            "gh pr diff 5",
            "gh pr checks 5",
            "gh api repos/o/r/pulls",
            "gh issue comment 5 -b hi",
        ] {
            assert!(!denied(command), "{command} must stay allowed");
        }
    }
}
