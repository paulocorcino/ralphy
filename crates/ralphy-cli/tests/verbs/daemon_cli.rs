//! `ralphy daemon setup --name --avatar`: the store verb a remote shell runs
//! over SSH, where nobody answers a prompt. Each test points
//! `RALPHY_DAEMON_DIR` at its own temp store on the CHILD only.

use std::path::Path;
use std::process::{Command, Output, Stdio};

fn daemon(store: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .arg("daemon")
        .args(args)
        .env("RALPHY_DAEMON_DIR", store)
        .stdin(Stdio::null())
        .output()
        .expect("spawn ralphy")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn setup_with_flags_needs_no_stdin_and_prints_no_secret() {
    let store = tempfile::tempdir().unwrap();
    let out = daemon(store.path(), &["setup", "--name", "anvil", "--avatar", "1"]);
    let stdout = text(&out.stdout);
    assert!(
        out.status.success(),
        "stdout: {stdout}\nstderr: {}",
        text(&out.stderr)
    );

    let toml = std::fs::read_to_string(store.path().join("daemon.toml")).unwrap();
    assert!(toml.contains("name = \"anvil\""), "daemon.toml: {toml}");
    assert!(toml.contains("avatar = "), "daemon.toml: {toml}");
    for secret in ["otpauth", "secret", "access token"] {
        assert!(
            !stdout.contains(secret),
            "setup with flags must print no {secret:?}: {stdout}"
        );
    }
    assert!(
        !store.path().join("daemon-totp").exists(),
        "setup with flags enrols no sign-in code"
    );
    assert!(
        !store.path().join("daemon-token").exists(),
        "setup with flags mints no access token"
    );
}

#[test]
fn setup_with_flags_refuses_a_reserved_name() {
    let store = tempfile::tempdir().unwrap();
    let out = daemon(store.path(), &["setup", "--name", "run", "--avatar", "1"]);
    assert!(!out.status.success(), "a reserved name must fail");
    let stderr = text(&out.stderr);
    assert!(stderr.contains("reserved"), "stderr: {stderr}");
    assert!(
        !store.path().join("daemon.toml").exists(),
        "a refused name writes no identity"
    );
}
