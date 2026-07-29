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
    // `ping -n 31` runs ~30 s and, unlike `timeout /t`, does not refuse a
    // redirected stdin — `spawn_detached` gives the child `Stdio::null()`, and a
    // child that exits instantly would let a `.wait()` implementation pass.
    let argv = vec![
        "ping".to_string(),
        "-n".to_string(),
        "31".to_string(),
        "127.0.0.1".to_string(),
    ];
    let t0 = std::time::Instant::now();
    spawn_detached(&argv).expect("ping is present on every Windows host");
    let elapsed = t0.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "spawn_detached returned in {elapsed:?} — it must not wait on the child"
    );

    // The oracle only holds if that child really does outlive the call: prove it
    // by running the SAME argv to completion and showing it takes far longer.
    let t1 = std::time::Instant::now();
    let status = std::process::Command::new("ping")
        .args(["-n", "31", "127.0.0.1"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("ping must run");
    let waited = t1.elapsed();
    assert!(status.success(), "the control child must exit cleanly");
    assert!(
        waited > std::time::Duration::from_secs(20),
        "the control child ran for {waited:?} — too short to distinguish waiting \
         from not waiting, so the assertion above proves nothing"
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
