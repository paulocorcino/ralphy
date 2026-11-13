//! `ralphy antigravity prepare-root` end to end: the real binary, run the way
//! the daemon runs it before an Antigravity console, with the Ralphy folder in
//! a temp dir so the operator's own root is never touched.

#[test]
fn prepare_root_writes_the_agy_hooks_file() {
    let store = tempfile::tempdir().unwrap();
    let out = crate::support::ralphy_command()
        .args(["antigravity", "prepare-root"])
        .env("RALPHY_DAEMON_DIR", store.path())
        .current_dir(store.path())
        .output()
        .expect("spawning ralphy");

    assert!(
        out.status.success(),
        "prepare-root must succeed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty(), "nothing on stdout: {:?}", out.stdout);
    let hooks = store
        .path()
        .join("antigravity-root")
        .join("config")
        .join("hooks.json");
    let text = std::fs::read_to_string(&hooks).expect("the hooks file is written");
    assert!(
        text.contains("guard"),
        "the guard hook is in the file: {text}"
    );
}
