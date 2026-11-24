//! `ralphy daemon add --init` on a host where git has no name and email. Each
//! test gives the CHILD its own empty global git config, so the real user's
//! config is never read or written.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use ralphy_daemon::dispatch::GIT_IDENTITY_MISSING;

const IDENTITY_ENV: [&str; 5] = [
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "EMAIL",
];

/// `ralphy daemon add` with no git identity: an empty global config, no system
/// config, and `user.useConfigOnly` so git does not guess one from the host.
fn daemon_add(home: &Path, args: &[&str]) -> Output {
    let mut cmd: Command = crate::support::ralphy_command();
    for key in IDENTITY_ENV {
        cmd.env_remove(key);
    }
    cmd.args(["daemon", "add"])
        .args(args)
        .env("RALPHY_DAEMON_DIR", home.join("store"))
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", home.join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "user.useConfigOnly")
        .env("GIT_CONFIG_VALUE_0", "true")
        .env("GIT_CONFIG_KEY_1", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_1", "false")
        .stdin(Stdio::null())
        .output()
        .expect("spawn ralphy")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_missing_git_identity_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("setup");
    let dir_arg = dir.to_str().unwrap();

    let out = daemon_add(home.path(), &["--init", "--create", "--", dir_arg]);

    assert!(!out.status.success(), "a missing identity must fail");
    let stderr = text(&out.stderr);
    assert!(stderr.contains(GIT_IDENTITY_MISSING), "stderr: {stderr}");
    assert!(!dir.exists(), "no folder was created");
}

#[test]
fn the_git_identity_flags_save_it_globally_and_create_the_repo() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("setup");
    let dir_arg = dir.to_str().unwrap();
    let args = [
        "--init",
        "--create",
        "--git-name",
        "Ana Lima",
        "--git-email",
        "ana@example.com",
        "--",
        dir_arg,
    ];

    let out = daemon_add(home.path(), &args);

    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let config = std::fs::read_to_string(home.path().join("gitconfig")).unwrap();
    assert!(config.contains("name = Ana Lima"), "gitconfig: {config}");
    assert!(
        config.contains("email = ana@example.com"),
        "gitconfig: {config}"
    );
    let author = crate::support::git_output(&dir, &["log", "-1", "--format=%an <%ae>"]);
    assert_eq!(author.trim(), "Ana Lima <ana@example.com>");
}
