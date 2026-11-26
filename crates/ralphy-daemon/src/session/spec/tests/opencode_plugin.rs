//! The state plugin an `OpenCode` console loads, by the version of its program.

use super::*;

/// The env of an `OpenCode` console whose program reads as `version`.
fn opencode_env_with_version(
    version: Result<&str, ()>,
) -> (Vec<(OsString, OsString)>, PathBuf, tempfile::TempDir) {
    let store = tempfile::tempdir().unwrap();
    let files = crate::agent_state::StatusFiles::for_agent(Agent::OpenCode, store.path(), 1);
    let mut spec = SessionSpec {
        program: OsString::from("opencode"),
        args: Vec::new(),
        cwd: store.path().to_path_buf(),
        rows: 24,
        cols: 80,
        env: Vec::new(),
        name: None,
        status: None,
        unreported_reason: None,
    };
    let read = |_: &Path| {
        version
            .map(|text| ralphy_proc_util::opencode::parse_opencode_version(text).unwrap())
            .map_err(|()| OpenCodeVersionError::Unreadable {
                program: OsString::from("opencode"),
            })
    };
    add_opencode_launch_with(&mut spec, files, read);
    let dir = store.path().to_path_buf();
    (spec.env, dir, store)
}

fn config_plugin(env: &[(OsString, OsString)]) -> Option<String> {
    let (_, text) = env.iter().find(|(k, _)| k == "OPENCODE_CONFIG_CONTENT")?;
    let cfg: serde_json::Value = serde_json::from_str(text.to_str()?).unwrap();
    cfg["plugin"][0].as_str().map(str::to_owned)
}

#[test]
fn a_2x_console_loads_the_2x_plugin() {
    let (env, dir, _guard) = opencode_env_with_version(Ok("2.3.2"));
    let folder = dir.join("1.state-plugin-v2");
    let entry = config_plugin(&env).expect("a plugin entry");
    assert_eq!(entry, crate::agent_state::opencode::plugin_entry(&folder));
    assert!(folder.join("index.js").is_file());
    assert!(folder.join("package.json").is_file());
    assert!(!dir.join("1.state-plugin.js").exists());
}

#[test]
fn a_1x_console_keeps_the_file_plugin() {
    let (env, dir, _guard) = opencode_env_with_version(Ok("1.19.0"));
    let file = dir.join("1.state-plugin.js");
    let entry = config_plugin(&env).expect("a plugin entry");
    assert_eq!(entry, crate::agent_state::opencode::plugin_entry(&file));
    assert!(file.is_file());
    assert!(!dir.join("1.state-plugin-v2").exists());
}

#[test]
fn a_console_with_an_unreadable_version_still_launches() {
    let (env, _dir, _guard) = opencode_env_with_version(Err(()));
    assert!(config_plugin(&env).is_none(), "{env:?}");
    assert!(env.iter().any(|(k, _)| k == "PWD"), "{env:?}");
}
