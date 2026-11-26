use super::*;
use crate::cleanup::remove_scratch;
use ralphy_proc_util::opencode::OpenCodeVersion;
use std::path::PathBuf;

pub(crate) fn squeezed(source: &str) -> String {
    source.split_whitespace().collect()
}

/// The program of every command is `located.program`, and every caller gets
/// it from the shared locator through [`checked_opencode`].
// pin: every opencode command spawns the program of the shared locator —
// the locator finds the same binary on a test machine, so no behavior test sees it.
#[test]
fn the_program_comes_from_the_shared_opencode_locator() {
    let command = squeezed(include_str!("../command.rs"));
    assert!(
        command.contains("ralphy_proc_util::opencode::located_opencode()"),
        "the locator must come from proc-util"
    );
    assert_eq!(
        command.matches("Command::new(").count(),
        command.matches("Command::new(&located.program)").count(),
        "every command must spawn the located program"
    );
    let lib = squeezed(include_str!("../lib.rs"));
    assert!(
        lib.contains("checked_opencode()"),
        "the run gets the checked version"
    );
    for located in [OpenCodeMajor::V1, OpenCodeMajor::V2].map(located) {
        let cmd = build_opencode_command(&located, run_args(None, None, Path::new("/r"), None));
        assert_eq!(cmd.get_program(), located.program.as_os_str());
        let models = build_models_command(&located);
        assert_eq!(models.get_program(), located.program.as_os_str());
    }
}

/// A located `OpenCode` of `major`, at a path no locator returns.
pub(crate) fn located(major: OpenCodeMajor) -> LocatedOpenCode {
    let text = match major {
        OpenCodeMajor::V1 => "1.19.0",
        OpenCodeMajor::V2 => "2.3.2",
    };
    let version = OpenCodeVersion {
        major,
        text: text.to_owned(),
    };
    LocatedOpenCode {
        program: "/fake/bin/opencode".into(),
        version,
    }
}

fn run_args<'a>(
    model: Option<&'a str>,
    variant: Option<&'a str>,
    root: &'a Path,
    skills_dir: Option<&'a Path>,
) -> RunArgs<'a> {
    RunArgs {
        model,
        variant,
        root,
        skills_dir,
    }
}

fn v1_command(m: Option<&str>, v: Option<&str>, root: &Path, skills: Option<&Path>) -> Command {
    build_opencode_command(&located(OpenCodeMajor::V1), run_args(m, v, root, skills))
}

fn v1_watched(root: &Path, state: &StatePlugin<'_>) -> Command {
    let run = run_args(None, None, root, None);
    build_watched_opencode_command(&located(OpenCodeMajor::V1), run, state)
}

pub(crate) fn argv(cmd: &Command) -> Vec<String> {
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
    for cmd in [v1_command(None, None, root, None), v1_watched(root, &state)] {
        let pwd = cmd.get_envs().find(|(k, _)| *k == "PWD");
        assert_eq!(
            pwd,
            Some((std::ffi::OsStr::new("PWD"), Some(root.as_os_str())))
        );
    }
}

/// The 1.x argv with no model and no variant: `-m` and `--variant` only when
/// set (ADR-0005 D3, D4); the logs on stderr for the limit detector (D9).
#[test]
fn build_command_omits_model_when_none() {
    let cmd = v1_command(None, None, Path::new("/repo"), None);
    let program = PathBuf::from(cmd.get_program());
    assert_eq!(
        program.file_stem().and_then(|s| s.to_str()),
        Some("opencode")
    );
    let base = "run --format json --dangerously-skip-permissions --print-logs --log-level ERROR";
    assert_eq!(argv(&cmd), strings(base));
}

#[test]
fn build_command_includes_model_when_some() {
    let cmd = v1_command(
        Some("anthropic/claude-sonnet-4-6"),
        None,
        Path::new("/repo"),
        None,
    );
    let args = argv(&cmd);
    assert_eq!(
        args[args.len() - 2..],
        strings("-m anthropic/claude-sonnet-4-6")
    );
}

#[test]
fn build_command_includes_variant_only_when_some() {
    let without = v1_command(None, None, Path::new("/repo"), None);
    assert!(!argv(&without).contains(&"--variant".to_owned()));
    let args = argv(&v1_command(None, Some("high"), Path::new("/repo"), None));
    assert_eq!(args[args.len() - 2..], strings("--variant high"));
}

#[test]
fn build_command_removes_both_api_keys() {
    let cmd = v1_command(None, None, Path::new("/repo"), None);
    for key in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"] {
        let removed = cmd.get_envs().any(|(k, v)| k == key && v.is_none());
        assert!(removed, "{key} should be removed on the child");
    }
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
    let cmd = v1_command(None, None, Path::new("/repo"), Some(&dir));
    let expected = dir.canonicalize().unwrap_or_else(|_| dir.clone());
    assert_eq!(
        injected_config(&cmd)["skills"]["paths"],
        serde_json::json!([expected])
    );

    let cfg = injected_config(&v1_command(None, None, Path::new("/repo"), None));
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
    let cmd = v1_watched(Path::new("/repo"), &state);
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

    let task = v1_command(None, None, Path::new("/repo"), None);
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
        let cmd = v1_command(None, None, Path::new("/repo"), skills_dir);
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

pub(crate) fn strings(words: &str) -> Vec<String> {
    words.split_whitespace().map(str::to_owned).collect()
}

fn v2_command(m: Option<&str>, v: Option<&str>) -> Command {
    let run = run_args(m, v, Path::new("/repo"), None);
    build_opencode_command(&located(OpenCodeMajor::V2), run)
}

/// The keys every command removes and the folder it runs in, on both majors.
fn assert_common_parts(cmd: &Command) {
    assert_eq!(cmd.get_current_dir(), Some(Path::new("/repo")));
    for key in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"] {
        assert!(
            cmd.get_envs().any(|(k, v)| k == key && v.is_none()),
            "{key}"
        );
    }
    let cfg = injected_config(cmd);
    assert_eq!(cfg["permission"]["bash"]["git push"], "deny", "{cfg}");
}

#[test]
fn the_1x_argv_is_unchanged() {
    let cmd = v1_command(Some("p/m"), Some("high"), Path::new("/repo"), None);
    assert_eq!(
        argv(&cmd),
        strings(concat!(
            "run --format json --dangerously-skip-permissions ",
            "--print-logs --log-level ERROR -m p/m --variant high"
        ))
    );
    assert_common_parts(&cmd);
}

#[test]
fn the_2x_argv_runs_standalone_with_auto() {
    let cmd = v2_command(Some("p/m"), Some("high"));
    assert_eq!(
        argv(&cmd),
        strings("run --format json --standalone --auto --print-logs --log-level error -m p/m#high")
    );
    assert_common_parts(&cmd);
    assert_eq!(
        argv(&v2_command(Some("p/m"), None))
            .last()
            .map(String::as_str),
        Some("p/m")
    );
}

#[test]
fn a_2x_variant_without_a_model_is_not_passed() {
    let args = argv(&v2_command(None, Some("high")));
    assert_eq!(args.last().map(String::as_str), Some("error"), "{args:?}");
    assert!(!args
        .iter()
        .any(|a| a == "-m" || a == "--variant" || a.contains('#')));
}

/// Keeps the formatted fields of each `info` event.
struct InfoLines(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for InfoLines {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        struct Text(String);
        impl tracing::field::Visit for Text {
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                use std::fmt::Write as _;
                write!(self.0, "{}={v:?} ", f.name()).expect("a String takes any text");
            }
        }
        if *event.metadata().level() == tracing::Level::INFO {
            let mut text = Text(String::new());
            event.record(&mut text);
            if let Ok(mut lines) = self.0.lock() {
                lines.push(text.0);
            }
        }
    }
}

#[test]
fn a_dropped_2x_variant_is_logged() {
    use tracing_subscriber::layer::SubscriberExt;
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(InfoLines(std::sync::Arc::clone(&lines)));
    tracing::subscriber::with_default(subscriber, || {
        v2_command(None, Some("high"));
        v2_command(Some("p/m"), Some("high"));
        v1_command(None, Some("high"), Path::new("/repo"), None);
    });
    let lines = lines.lock().expect("no test thread panicked").clone();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("high") && lines[0].contains("OpenCode 2"),
        "{lines:?}"
    );
}

/// The 2.x command names the plugin folder under `plugin` (2.x loads a
/// folder) and gives the plugin the status file; the deny map stays.
#[test]
fn a_2x_command_names_the_plugin_folder() {
    let folder = Path::new("/run/ralphy-state-plugin-v2");
    let state = StatePlugin {
        plugin: folder,
        status: Path::new("/run/status"),
    };
    let run = run_args(None, None, Path::new("/repo"), None);
    let cmd = build_watched_opencode_command(&located(OpenCodeMajor::V2), run, &state);
    let cfg = injected_config(&cmd);
    assert_eq!(
        cfg["plugin"],
        serde_json::json!([plugin_entry(folder)]),
        "{cfg}"
    );
    let status = cmd
        .get_envs()
        .find(|(k, _)| *k == "RALPHY_STATUS_FILE")
        .and_then(|(_, v)| v);
    assert_eq!(status, Some(Path::new("/run/status").as_os_str()));
    assert_eq!(cfg["permission"]["bash"]["git push"], "deny");
}

/// The checked `OpenCode` of this host when it is 2.x; a live test skips otherwise.
fn live_2x() -> Option<LocatedOpenCode> {
    let located = checked_opencode().ok()?;
    (located.version.major == OpenCodeMajor::V2).then_some(located)
}

/// Every `{"resource": r, "effect": "deny"}` object anywhere in `value`.
fn denied_resources(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let (Some(r), Some("deny")) = (
                map.get("resource"),
                map.get("effect").and_then(|e| e.as_str()),
            ) {
                out.push(r.as_str().unwrap_or_default().to_owned());
            }
            map.values().for_each(|v| denied_resources(v, out));
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| denied_resources(v, out)),
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

#[test]
#[ignore = "live: runs the real opencode 2.x; run manually with -- --ignored"]
fn debug_config_of_2x_shows_the_deny_map() {
    let Some(located) = live_2x() else { return };
    let root = tempfile::tempdir().expect("a scratch folder");
    let built = build_opencode_command(&located, run_args(None, None, root.path(), None));
    let mut cmd = Command::new(built.get_program());
    cmd.args(["debug", "config", "--standalone"])
        .current_dir(root.path());
    for (key, value) in built.get_envs() {
        match value {
            Some(v) => cmd.env(key, v),
            None => cmd.env_remove(key),
        };
    }
    let out = cmd.output().expect("opencode debug config runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let config: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let mut denied = Vec::new();
    denied_resources(&config, &mut denied);
    for words in DENIED_FORGE_WRITES {
        let bare = words.join(" ");
        assert!(denied.contains(&bare), "{bare}: {denied:?}");
        assert!(
            denied.contains(&format!("{bare} *")),
            "{bare} *: {denied:?}"
        );
    }
}

#[test]
#[ignore = "live: runs the real opencode 2.x; run manually with -- --ignored"]
fn a_denied_call_is_refused_under_auto_on_2x() {
    use std::io::Write;
    let Some(located) = live_2x() else { return };
    let root = tempfile::tempdir().expect("a scratch folder");
    let init = Command::new("git").arg("init").arg(root.path()).output();
    assert!(init.is_ok_and(|o| o.status.success()), "git init");
    // A free model can hit its usage limit; the operator names a model of a provider instead.
    let model = std::env::var("RALPHY_LIVE_OPENCODE_MODEL")
        .unwrap_or_else(|_| "opencode/big-pickle".to_owned());
    let run = run_args(Some(&model), None, root.path(), None);
    let mut child = build_opencode_command(&located, run)
        .spawn()
        .expect("opencode runs");
    let prompt = "Run the shell command `git push --dry-run origin HEAD` and tell me its output.";
    let mut stdin = child.stdin.take().expect("stdin is piped");
    stdin
        .write_all(prompt.as_bytes())
        .expect("the prompt is written");
    drop(stdin);
    let out = child.wait_with_output().expect("opencode ends");
    let stream = String::from_utf8_lossy(&out.stdout);
    assert!(stream.contains("Permission denied"), "{stream}");
}
