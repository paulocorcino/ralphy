use super::*;
use ralphy_core::Usage;
use std::path::PathBuf;

fn plan_with(recommended: Option<&str>) -> Plan {
    Plan {
        path: PathBuf::from("/x/plan.md"),
        open_steps: 3,
        recommended_model: recommended.map(str::to_owned),
        usage: Usage::default(),
        session_id: None,
    }
}

fn agent_with(exec_model: Option<&str>, default: &str) -> ClaudeAgent {
    ClaudeAgent::new(None, None, PathBuf::from("/run")).with_exec_config(
        exec_model.map(str::to_owned),
        Some("medium".into()),
        default.to_owned(),
        45,
        true,
        false,
        6,
    )
}

#[test]
fn exec_config_default_remote_control_off() {
    assert!(!ExecConfig::default().remote_control);
}

#[test]
fn reads_recommended_model() {
    assert_eq!(
        recommended_model("## Execution model: Opus\nbecause").as_deref(),
        Some("opus")
    );
    assert_eq!(recommended_model("no judgment here"), None);
    // The effort-bearing rung must parse whole, not be shadowed by `opus`.
    assert_eq!(
        recommended_model("## Execution model: opus-high\nbecause").as_deref(),
        Some("opus-high")
    );
    assert_eq!(
        recommended_model("## Execution model: Opus-High\n").as_deref(),
        Some("opus-high")
    );
}

#[test]
fn opus_high_judgment_selects_the_opus_model() {
    // The rung carries effort separately; the `--model` argv is plain `opus`.
    let agent = agent_with(None, "sonnet");
    assert_eq!(
        agent.resolve_exec_model(&plan_with(Some("opus-high"))),
        "opus"
    );
}

#[test]
fn opus_high_judgment_couples_effort_to_high_when_operator_is_silent() {
    // No `--exec-effort`: the plan's opus-high rung raises effort to high.
    let silent = ClaudeAgent::new(None, None, PathBuf::from("/run")).with_exec_config(
        None,
        None, // operator did not name an exec effort
        "sonnet".into(),
        45,
        true,
        false,
        6,
    );
    assert_eq!(
        silent
            .resolve_exec_effort(&plan_with(Some("opus-high")))
            .as_deref(),
        Some("high")
    );
    // A plain opus (or sonnet) rung leaves effort absent — Claude's own default.
    assert_eq!(silent.resolve_exec_effort(&plan_with(Some("opus"))), None);
    assert_eq!(silent.resolve_exec_effort(&plan_with(None)), None);
}

#[test]
fn operator_exec_effort_wins_over_the_opus_high_rung() {
    // An explicit `--exec-effort medium` beats the plan's opus-high default.
    let forced = agent_with(None, "sonnet"); // agent_with sets exec_effort = medium
    assert_eq!(
        forced
            .resolve_exec_effort(&plan_with(Some("opus-high")))
            .as_deref(),
        Some("medium")
    );
}

#[test]
fn exec_model_explicit_override_wins() {
    let agent = agent_with(Some("opus"), "sonnet");
    assert_eq!(agent.resolve_exec_model(&plan_with(Some("sonnet"))), "opus");
}

#[test]
fn exec_model_falls_back_to_plan_judgment() {
    let agent = agent_with(None, "sonnet");
    assert_eq!(agent.resolve_exec_model(&plan_with(Some("opus"))), "opus");
}

#[test]
fn exec_model_falls_back_to_default() {
    let agent = agent_with(None, "sonnet");
    assert_eq!(agent.resolve_exec_model(&plan_with(None)), "sonnet");
}

/// ADR-0059 §4: the status hooks ride both phases; the guard and the
/// sentinel Stop keep their FIRST slot and their own command on execute;
/// the plan phase has the status hooks and nothing else; and the events
/// the ADR rejected are registered on neither.
#[test]
fn status_hooks_ride_both_phases_after_the_guard_and_the_sentinel() {
    let shell = |c: &str| serde_json::json!({ "type": "command", "command": c });
    let status = "\"ralphy.exe\" hook status";
    let exec: serde_json::Value = serde_json::from_str(&exec_settings_json(
        &shell("\"ralphy.exe\" hook stop"),
        &shell("\"ralphy.exe\" hook guard --cost-gate"),
        &shell("\"ralphy.exe\" hook post"),
        &shell(status),
    ))
    .unwrap();
    let plan: serde_json::Value = serde_json::from_str(&plan_settings_json(
        &shell("\"ralphy.exe\" hook guard"),
        &shell(status),
    ))
    .unwrap();
    for (doc, name) in [(&exec, "exec"), (&plan, "plan")] {
        let hooks = &doc["hooks"];
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PermissionRequest",
            "SubagentStop",
        ] {
            let list = hooks[event]
                .as_array()
                .unwrap_or_else(|| panic!("{name}: {event}"));
            assert_eq!(list.len(), 1, "{name}: one status hook on {event}");
            assert_eq!(list[0]["hooks"][0]["command"], status, "{name}: {event}");
        }
        assert_eq!(hooks["PermissionRequest"][0]["matcher"], "*", "{name}");
        for absent in [
            "Notification",
            "PreCompact",
            "PostToolUseFailure",
            "SubagentStart",
        ] {
            assert!(
                hooks.get(absent).is_none(),
                "{name}: {absent} must not be registered"
            );
        }
        for (key, want) in [
            ("skipDangerousModePermissionPrompt", true),
            ("skipAutoPermissionPrompt", true),
            ("autoCompactEnabled", false),
        ] {
            assert_eq!(doc[key], want, "{name}: {key}");
        }
    }
    // Execute: the guard is first on PreToolUse with its narrow matcher,
    // the status hook second with `*`; the sentinel first on Stop.
    let pre = exec["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 2);
    assert_eq!(
        pre[0]["matcher"],
        "Bash|PowerShell|Edit|Write|MultiEdit|NotebookEdit"
    );
    assert_eq!(
        pre[0]["hooks"][0]["command"],
        "\"ralphy.exe\" hook guard --cost-gate"
    );
    assert_eq!(pre[0]["hooks"][0]["type"], "command");
    assert_eq!(pre[1]["matcher"], "*");
    assert_eq!(pre[1]["hooks"][0]["command"], status);
    let stop = exec["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0]["hooks"][0]["command"], "\"ralphy.exe\" hook stop");
    assert_eq!(stop[0]["hooks"][0]["type"], "command");
    assert_eq!(stop[1]["hooks"][0]["command"], status);
    // The Bash timer first on PostToolUse, the status hook second with `*`.
    let post = exec["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 2);
    assert_eq!(post[0]["matcher"], "Bash");
    assert_eq!(post[0]["hooks"][0]["command"], "\"ralphy.exe\" hook post");
    assert_eq!(post[0]["hooks"][0]["type"], "command");
    assert_eq!(post[1]["matcher"], "*");
    assert_eq!(post[1]["hooks"][0]["command"], status);
    // Plan: the guard first, then the status hooks — no sentinel, no timer.
    let pre = plan["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 2);
    assert_eq!(
        pre[0]["matcher"],
        "Bash|PowerShell|Edit|Write|MultiEdit|NotebookEdit"
    );
    assert_eq!(pre[0]["hooks"][0]["command"], "\"ralphy.exe\" hook guard");
    assert_eq!(pre[1]["matcher"], "*");
    assert_eq!(pre[1]["hooks"][0]["command"], status);
    let stop = plan["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 1);
    assert_eq!(stop[0]["hooks"][0]["command"], status);
    let post = plan["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 1);
    assert_eq!(post[0]["matcher"], "*");
    assert_eq!(post[0]["hooks"][0]["command"], status);
}

/// The status hook events and their matchers are the shared list the
/// daemon's console settings are pinned by too.
#[test]
fn the_status_hooks_match_the_shared_event_list() {
    let shared: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../tests/fixtures/hook_events.json")).unwrap();
    let shared: Vec<(String, String)> = shared
        .iter()
        .map(|e| {
            (
                e["event"].as_str().unwrap().to_owned(),
                e["matcher"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let status = serde_json::json!({ "type": "command", "command": "ralphy hook status" });
    let ours: Vec<(String, String)> = status_hook_entries(&status)
        .into_iter()
        .map(|(event, entry)| {
            assert_eq!(entry["hooks"][0], status, "{event}");
            (
                event.to_owned(),
                entry["matcher"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(ours, shared);
}

/// ADR-0059 §4 as WRITTEN, not as built in memory: the files
/// `write_exec_settings`/`write_plan_settings` leave in the run dir carry
/// the status hook on every one of the seven events, and the plan file has
/// no sentinel. Dropping the `status_hook_command` argument
/// from either writer reds here where the pure-builder test stays green.
#[test]
fn the_written_settings_files_carry_the_status_hooks() {
    type Writer = fn(&ClaudeAgent) -> Result<SettingsFile>;
    let dir = tempfile::tempdir().unwrap();
    let agent = ClaudeAgent::new(None, None, dir.path().to_path_buf());
    // Both writers land on the SAME path (the run's one settings file), so
    // each is written and read back in turn.
    let writers: [(&str, Writer, bool); 2] = [
        ("exec", |a| a.write_exec_settings(), true),
        ("plan", |a| a.write_plan_settings(), false),
    ];
    for (name, write, sentinel_wanted) in writers {
        let path = write(&agent).unwrap().path;
        assert_eq!(path, dir.path().join("ralphy.settings.json"), "{name}");
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let hooks = doc["hooks"]
            .as_object()
            .unwrap_or_else(|| panic!("{name}: hooks"));
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
            "Stop",
            "SubagentStop",
        ] {
            let list = hooks[event]
                .as_array()
                .unwrap_or_else(|| panic!("{name}: {event}"));
            let status = list
                .iter()
                .map(|e| hook_text(&e["hooks"][0]))
                .filter(|c| c.ends_with("hook status"))
                .count();
            assert_eq!(status, 1, "{name}: one status hook on {event}");
        }
        let sentinel = hooks["Stop"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| hook_text(&e["hooks"][0]))
            .any(|c| c.ends_with("hook stop"));
        assert_eq!(sentinel, sentinel_wanted, "{name}: sentinel presence");
        // The command names THIS binary. On Windows the shell form (no
        // `claude` ≥ 2.1.139 on the machine) writes the path with `/`.
        let cmd = hook_text(&hooks["SessionStart"][0]["hooks"][0]).replace('\\', "/");
        let exe = hook_exe().unwrap().to_string_lossy().replace('\\', "/");
        assert!(cmd.contains(&exe), "{name}: {cmd}");
    }
}
/// What a hook entry runs, whatever its form: `command`, then `args`
/// joined by spaces.
fn hook_text(hook: &serde_json::Value) -> String {
    let mut parts = vec![hook["command"].as_str().unwrap_or_default().to_owned()];
    if let Some(args) = hook["args"].as_array() {
        parts.extend(args.iter().filter_map(|a| a.as_str().map(str::to_owned)));
    }
    parts.join(" ")
}

fn read_settings(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The guard command of a written settings file, if it carries one.
fn guard_command_of(path: &Path) -> Option<String> {
    read_settings(path)["hooks"]["PreToolUse"]
        .as_array()?
        .iter()
        .map(|e| hook_text(&e["hooks"][0]))
        .find(|c| c.contains("hook guard"))
}

/// Every settings file the Claude adapter writes carries the guard, and
/// only the execute file turns the cost gate on. The task sessions share
/// one writer: no task builds its own settings file.
#[test]
fn every_settings_file_carries_the_guard() {
    let dir = tempfile::tempdir().unwrap();
    let agent = ClaudeAgent::new(None, None, dir.path().join("run"));
    std::fs::create_dir_all(dir.path().join("run")).unwrap();
    let exec = agent.write_exec_settings().unwrap().path;
    let exec_guard = guard_command_of(&exec);
    let plan = agent.write_plan_settings().unwrap().path;
    let plan_guard = guard_command_of(&plan);
    let task = write_task_settings(&dir.path().join("task").join(".ralphy"))
        .unwrap()
        .path;
    let task_guard = guard_command_of(&task);
    for (name, got, want) in [
        ("exec", exec_guard, "hook guard --cost-gate"),
        ("plan", plan_guard, "hook guard"),
        ("task", task_guard, "hook guard"),
    ] {
        let got = got.unwrap_or_else(|| panic!("{name}: no guard hook"));
        assert!(got.ends_with(want), "{name}: {got}");
    }
    let tasks = include_str!("../tasks.rs");
    assert!(
        !tasks.contains("std::fs::write(&settings_path"),
        "a task session writes its own settings file"
    );
    assert_eq!(tasks.matches("write_task_settings(").count(), 4);
}

/// The waiver removes only the refusal: with it on, the exec, plan and
/// task files still carry the guard and the status hooks.
#[test]
fn the_waiver_keeps_every_hook() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("run")).unwrap();
    let agent = ClaudeAgent::new(None, None, dir.path().join("run")).with_guard_proof_waiver(true);
    assert!(agent.exec.run_without_guard_proof);
    let exec = agent.write_exec_settings().unwrap().path;
    let exec = (guard_command_of(&exec), read_settings(&exec));
    let plan = agent.write_plan_settings().unwrap().path;
    let plan = (guard_command_of(&plan), read_settings(&plan));
    let task = write_task_settings(&dir.path().join("task").join(".ralphy"))
        .unwrap()
        .path;
    let task = (guard_command_of(&task), read_settings(&task));
    for (name, (guard, doc)) in [("exec", exec), ("plan", plan), ("task", task)] {
        assert!(guard.is_some(), "{name}: no guard hook");
        let status = doc["hooks"]["SessionStart"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: SessionStart"))
            .iter()
            .any(|e| hook_text(&e["hooks"][0]).ends_with("hook status"));
        assert!(status, "{name}: no status hook");
    }
}

fn expected_entry(args: &[&str]) -> serde_json::Value {
    hook_command(hook_exe().unwrap(), args).entry()
}

/// Each hook in a built file is exactly the entry the module chose.
#[test]
fn the_settings_carry_the_entry_the_module_chose() {
    let dir = tempfile::tempdir().unwrap();
    let agent = ClaudeAgent::new(None, None, dir.path().to_path_buf());
    let status = expected_entry(&["hook", "status"]);
    let exec = read_settings(&agent.write_exec_settings().unwrap().path);
    let want = [
        ("Stop", 0, expected_entry(&["hook", "stop"])),
        ("Stop", 1, status.clone()),
        (
            "PreToolUse",
            0,
            expected_entry(&["hook", "guard", "--cost-gate"]),
        ),
        ("PreToolUse", 1, status.clone()),
        ("PostToolUse", 0, expected_entry(&["hook", "post"])),
        ("PostToolUse", 1, status.clone()),
        ("SessionStart", 0, status.clone()),
    ];
    for (event, slot, entry) in want {
        assert_eq!(
            exec["hooks"][event][slot]["hooks"][0], entry,
            "{event} {slot}"
        );
    }
    let plan = read_settings(&agent.write_plan_settings().unwrap().path);
    assert_eq!(
        plan["hooks"]["PreToolUse"][0]["hooks"][0],
        expected_entry(&["hook", "guard"])
    );
    assert_eq!(plan["hooks"]["Stop"][0]["hooks"][0], status);
    let form = hook_command(hook_exe().unwrap(), &["hook", "status"]).form();
    let entry = &status;
    match form {
        HookForm::Exec => {
            let args = entry["args"].as_array().unwrap();
            assert_eq!(&args[args.len() - 2..], ["hook", "status"]);
        }
        HookForm::Shell => assert!(entry["command"].as_str().unwrap().contains("hook")),
    }
}

/// A task session proves its hooks on `SessionStart`: exactly one status
/// hook there, besides the guard.
#[test]
fn the_task_settings_carry_the_session_start_hook() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_task_settings(dir.path()).unwrap();
    let doc = read_settings(&file.path);
    let start = doc["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(start.len(), 1);
    assert_eq!(start[0]["matcher"], "");
    assert!(hook_text(&start[0]["hooks"][0]).ends_with("hook status"));
    let hooks = doc["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), 2, "only PreToolUse and SessionStart");
    assert_eq!(hooks["PreToolUse"].as_array().unwrap().len(), 1);
}

/// `unproven` carries the module's reason for a shell form, and is `None`
/// for the exec form.
#[test]
fn a_shell_form_settings_file_says_it_is_unproven() {
    let exe = Path::new("ralphy");
    let shell = ralphy_proc_util::hook_form::hook_for_version(Some("2.1.138"), exe, &["hook"]);
    let exec = ralphy_proc_util::hook_form::hook_for_version(Some("2.1.139"), exe, &["hook"]);
    assert_eq!(shell.form(), HookForm::Shell);
    assert_eq!(exec.form(), HookForm::Exec);
    let path = PathBuf::from("x.json");
    let mixed = SettingsFile::new(path.clone(), [&exec, &shell]);
    assert_eq!(mixed.unproven.as_deref(), Some(shell.reason()));
    let proven = SettingsFile::new(path, [&exec, &exec]);
    assert_eq!(proven.unproven, None);
}
