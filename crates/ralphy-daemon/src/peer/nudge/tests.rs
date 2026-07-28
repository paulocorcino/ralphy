use super::*;

fn spec() -> NudgeSpec {
    NudgeSpec {
        distro: "Ubuntu-22.04".into(),
        unit: "ralphy-daemon.service".into(),
    }
}

#[test]
fn nudge_argv_is_exact() {
    assert_eq!(
        nudge_argv(&spec()),
        vec![
            "wsl.exe",
            "-d",
            "Ubuntu-22.04",
            "-e",
            "systemctl",
            "--user",
            "start",
            "ralphy-daemon.service",
        ]
    );
}

#[test]
fn nudge_argv_has_no_shell_metacharacters() {
    for arg in nudge_argv(&spec()) {
        assert!(
            !arg.contains([' ', '"', '|', '&', ';']),
            "argv element `{arg}` would need quoting — the nudge must never be a shell string"
        );
    }
}

/// The nudger never waits: spawning a process that outlives the call must return
/// immediately, not block for the child's lifetime.
#[cfg(windows)]
#[test]
fn nudge_never_waits() {
    let argv = vec![
        "cmd.exe".to_string(),
        "/c".to_string(),
        "timeout".to_string(),
        "/t".to_string(),
        "30".to_string(),
    ];
    let t0 = std::time::Instant::now();
    spawn_detached(&argv).expect("cmd.exe is present on every Windows host");
    let elapsed = t0.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "spawn_detached returned in {elapsed:?} — it must not wait on the child"
    );
}

/// Off Windows there is no `wsl.exe`, so the nudge is a legible refusal.
#[cfg(not(windows))]
#[test]
fn nudge_off_windows_refuses() {
    let err = spawn_detached(&nudge_argv(&spec())).expect_err("must refuse off Windows");
    assert!(
        err.to_string()
            .contains("only available from a Windows host"),
        "got: {err}"
    );
}
