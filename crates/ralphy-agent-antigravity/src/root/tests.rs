use super::*;

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// The `command` strings of one event's handlers, matcher entries flattened.
fn commands(event: &Value) -> Vec<String> {
    let entries = event.as_array().expect("an event is a list");
    entries
        .iter()
        .flat_map(|entry| match entry.get("hooks") {
            Some(inner) => inner.as_array().unwrap().clone(),
            None => vec![entry.clone()],
        })
        .map(|handler| {
            assert_eq!(handler["type"], "command", "{handler}");
            handler["command"].as_str().unwrap().to_owned()
        })
        .collect()
}

#[test]
fn the_owned_root_gets_the_hooks_file_with_the_guard_on_every_tool() {
    let root = tempfile::tempdir().unwrap();
    ensure(root.path()).unwrap();

    let ralphy = ralphy_proc_util::exe::running_exe().unwrap();
    let hook = |args: &[&str]| ralphy_proc_util::antigravity::hook_command(&ralphy, args).unwrap();
    let guard = hook(&["hook", "guard", "--vendor", "antigravity"]);
    let status = hook(&["hook", "status", "antigravity"]);

    let doc = read_json(&root.path().join("config").join("hooks.json"));
    let events = doc["ralphy"].as_object().expect("one named hook: ralphy");
    assert_eq!(doc.as_object().unwrap().len(), 1, "{doc}");
    assert!(
        doc.get("hooks").is_none(),
        "a top-level `hooks` key runs nothing"
    );

    let pre = &events["PreToolUse"];
    assert_eq!(pre[0]["matcher"], "*", "{pre}");
    assert_eq!(
        commands(pre),
        [guard, status.clone()],
        "the guard runs first"
    );
    assert_eq!(events["PostToolUse"][0]["matcher"], "*");

    let mut names: Vec<&str> = events.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "PostInvocation",
            "PostToolUse",
            "PreInvocation",
            "PreToolUse",
            "SessionStart",
            "Stop"
        ]
    );
    for (name, event) in events {
        assert!(
            commands(event).contains(&status),
            "no status hook on {name}"
        );
    }
    for flat in ["SessionStart", "PreInvocation", "PostInvocation", "Stop"] {
        assert!(
            events[flat][0].get("matcher").is_none(),
            "{flat} is a flat list"
        );
    }
}

#[test]
fn the_owned_root_turns_swarm_off_and_keeps_other_keys() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    fs::create_dir_all(&config).unwrap();
    let before = json!({
        "theme": "dark",
        "plugins": {
            "swarm": { "enabled": true, "version": 3 },
            "other": { "enabled": true },
        },
    });
    fs::write(config.join("config.json"), before.to_string()).unwrap();

    ensure(root.path()).unwrap();

    let after = read_json(&config.join("config.json"));
    assert_eq!(
        after,
        json!({
            "theme": "dark",
            "plugins": {
                "swarm": { "enabled": false, "version": 3 },
                "other": { "enabled": true },
            },
        })
    );

    let fresh = tempfile::tempdir().unwrap();
    ensure(fresh.path()).unwrap();
    assert_eq!(
        read_json(&fresh.path().join("config").join("config.json")),
        json!({ "plugins": { "swarm": { "enabled": false } } })
    );
}

#[test]
fn a_workspace_hooks_file_is_found_up_to_the_repo_root() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let cwd = repo.join("sub").join("dir");
    fs::create_dir_all(&cwd).unwrap();
    assert_eq!(workspace_hooks_file(&cwd, &repo), None);

    let plant = |dir: &Path| {
        fs::create_dir_all(dir).unwrap();
        let file = dir.join("hooks.json");
        fs::write(&file, "{}").unwrap();
        file
    };
    // Above the repository root: not the repository's file.
    plant(&tmp.path().join(".agents"));
    assert_eq!(workspace_hooks_file(&cwd, &repo), None);

    for name in [".agents", ".agent", "_agents"] {
        let at_root = plant(&repo.join(name));
        assert_eq!(
            workspace_hooks_file(&cwd, &repo),
            Some(at_root.clone()),
            "{name}"
        );
        fs::remove_file(&at_root).unwrap();
    }

    let at_root = plant(&repo.join(".agents"));
    let nearer = plant(&repo.join("sub").join("_agents"));
    assert_eq!(workspace_hooks_file(&cwd, &repo), Some(nearer));
    assert_eq!(workspace_hooks_file(&repo, &repo), Some(at_root));
}
