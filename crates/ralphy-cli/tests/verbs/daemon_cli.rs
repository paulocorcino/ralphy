//! `ralphy daemon setup --name --avatar` and `ralphy daemon require-token`: the
//! store verbs a remote shell runs over SSH, where nobody answers a prompt. Each
//! test points `RALPHY_DAEMON_DIR` at its own temp store on the CHILD only.

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

#[test]
fn require_token_on_then_off() {
    let store = tempfile::tempdir().unwrap();
    let marker = store.path().join("daemon-require-token");
    let token_file = store.path().join("daemon-token");

    let on = daemon(store.path(), &["require-token", "on"]);
    let stdout = text(&on.stdout);
    assert!(on.status.success(), "stderr: {}", text(&on.stderr));
    assert!(marker.exists(), "`on` writes the marker");
    let token = std::fs::read_to_string(&token_file).expect("`on` creates the access token");
    let token = token.trim();
    assert!(!token.is_empty());
    assert!(
        !stdout.contains(token),
        "the token is never printed: {stdout}"
    );
    assert!(stdout.contains("ralphy daemon restart"), "stdout: {stdout}");

    let again = daemon(store.path(), &["require-token", "on"]);
    let stdout = text(&again.stdout);
    assert!(again.status.success(), "stderr: {}", text(&again.stderr));
    assert!(stdout.contains("already set"), "stdout: {stdout}");
    assert!(!stdout.contains(token), "stdout: {stdout}");

    let off = daemon(store.path(), &["require-token", "off"]);
    assert!(off.status.success(), "stderr: {}", text(&off.stderr));
    assert!(!marker.exists(), "`off` removes the marker");
    assert!(token_file.exists(), "`off` keeps the access token");
}

fn json(out: &Output) -> serde_json::Value {
    assert!(out.status.success(), "stderr: {}", text(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("describe prints one JSON object")
}

#[test]
fn describe_prints_the_pairing_facts() {
    let store = tempfile::tempdir().unwrap();
    let setup = daemon(store.path(), &["setup", "--name", "anvil", "--avatar", "1"]);
    assert!(setup.status.success(), "stderr: {}", text(&setup.stderr));
    let on = daemon(store.path(), &["require-token", "on"]);
    assert!(on.status.success(), "stderr: {}", text(&on.stderr));
    let token = std::fs::read_to_string(store.path().join("daemon-token")).unwrap();

    let d = json(&daemon(store.path(), &["describe", "--with-token"]));
    assert_eq!(d["name"], "anvil", "{d}");
    assert_eq!(d["require_token"], true, "{d}");
    assert_eq!(
        d["protocol_version"],
        ralphy_daemon::peer::PEER_PROTOCOL_VERSION,
        "{d}"
    );
    assert_eq!(d["port"], 7257, "{d}");
    assert!(d.get("socket").is_none(), "no daemon, no socket: {d}");
    assert_eq!(d["token"], token.trim(), "{d}");
    assert!(
        d["daemon_id"].as_str().is_some_and(|s| !s.is_empty()),
        "{d}"
    );

    let d = json(&daemon(store.path(), &["describe"]));
    assert!(
        d.get("token").is_none(),
        "no token without --with-token: {d}"
    );
}

#[test]
fn rotate_token_changes_the_token() {
    let store = tempfile::tempdir().unwrap();
    let token_file = store.path().join("daemon-token");
    let on = daemon(store.path(), &["require-token", "on"]);
    assert!(on.status.success(), "stderr: {}", text(&on.stderr));
    let before = std::fs::read_to_string(&token_file).unwrap();

    let out = daemon(store.path(), &["rotate-token"]);
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "stderr: {}", text(&out.stderr));
    let after = std::fs::read_to_string(&token_file).unwrap();
    assert_ne!(before.trim(), after.trim());
    assert!(!after.trim().is_empty());
    assert!(
        !stdout.contains(after.trim()),
        "the token is never printed: {stdout}"
    );
    assert!(stdout.contains("ralphy daemon restart"), "stdout: {stdout}");
}
