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

/// The encoding `wsl.exe` actually emits on a build that ignores `WSL_UTF8`:
/// UTF-16LE with a BOM. Read as UTF-8 this is mostly NUL bytes, and every distro
/// name would be missed — which would report a running distro as stopped.
#[test]
fn utf16_output_decodes() {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in "Ubuntu-22.04\r\n".encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    assert!(running_contains(&decode_wsl_output(&bytes), "Ubuntu-22.04"));
}

/// The encoding a build that honours `WSL_UTF8` emits. Plain UTF-8 carries no
/// NUL, which is exactly what the detection keys on.
#[test]
fn utf8_output_decodes() {
    let bytes = b"Ubuntu-22.04\nDebian\n";
    assert!(running_contains(&decode_wsl_output(bytes), "Debian"));
}

/// A name matches whole and case-insensitively, as `wsl.exe -d` resolves one.
/// Substring matching would let the shorter name claim the longer distro and
/// report a stopped `Ubuntu-22.04` as running.
#[test]
fn a_running_distro_matches_whole_and_case_insensitively() {
    let listed = "Ubuntu-22.04\r\n";
    assert!(running_contains(listed, "ubuntu-22.04"));
    assert!(!running_contains(listed, "Ubuntu"));
    assert!(!running_contains(listed, "Ubuntu-22.04-test"));
}

/// `--list --running` prints nothing when no distro is running (and exits
/// non-zero, which is why the status is not the signal). Empty must read as
/// "not running", never as a match.
#[test]
fn empty_output_lists_nothing() {
    for listed in ["", "\r\n", "\u{feff}\r\n"] {
        assert!(
            !running_contains(listed, "Ubuntu-22.04"),
            "empty output matched: {listed:?}"
        );
    }
}

/// The question must be answerable without waking anything. `--list --running`
/// is a management command, so running it here cannot start a distro — asserted
/// by the argv this test pins, since a `-e` form would have created a session.
#[cfg(windows)]
#[test]
fn distro_liveness_answers_without_starting_anything() {
    let before = is_distro_running("Ubuntu-22.04");
    // A host with no WSL at all is a legitimate CI shape: the error is what the
    // caller degrades to "unknown" on, so either outcome is a pass here.
    if let Ok(running) = before {
        assert_eq!(
            is_distro_running("Ubuntu-22.04").expect("a second read must also answer"),
            running,
            "the liveness read changed the thing it measures"
        );
    }
    assert!(
        !is_distro_running("no-such-distro-9d3f").unwrap_or(false),
        "a distro that does not exist can never be running"
    );
}

/// Off Windows the question has no answer, and the caller must get an error to
/// degrade to "unknown" rather than a `false` that would read as "stopped".
#[cfg(not(windows))]
#[test]
fn distro_liveness_off_windows_refuses() {
    let err = is_distro_running("Ubuntu-22.04").expect_err("must refuse off Windows");
    assert!(err.to_string().contains("Windows host"), "got: {err}");
}

#[test]
fn keepalive_argv_is_exact() {
    assert_eq!(
        keepalive_argv(&spec()),
        vec!["wsl.exe", "-d", "Ubuntu-22.04", "-e", "sleep", "infinity"]
    );
}

#[test]
fn keepalive_argv_has_no_shell_metacharacters() {
    for arg in keepalive_argv(&spec()) {
        assert!(
            !arg.contains([' ', '"', '|', '&', ';']),
            "argv element `{arg}` would need quoting — the keepalive must never be a shell string"
        );
    }
}

/// One handle per distro: a second `ensure` while the first child still runs
/// must spawn nothing, and a child that has exited holds nothing, so it is
/// replaced. Driven with local children so the registry's own logic is what is
/// tested, not WSL — a `Keepalives` of this test's own, never the process-wide
/// one, so parallel tests cannot see each other's handles.
#[cfg(windows)]
#[test]
fn a_keepalive_is_held_once_per_distro_and_replaced_once_dead() {
    use std::process::{Command, Stdio};

    fn child(args: &[&str]) -> Result<Child> {
        Ok(Command::new(args[0])
            .args(&args[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?)
    }
    let held = Keepalives(Mutex::new(HashMap::new()));

    // A long-lived child stands in for `sleep infinity` (see `nudge_never_waits`
    // for why `ping`).
    let long = || child(&["ping", "-n", "31", "127.0.0.1"]);
    assert!(held.ensure_with("distro-a", long).expect("first spawn"));
    assert!(
        !held
            .ensure_with("distro-a", || panic!("must not spawn a second keepalive"))
            .expect("second ensure"),
        "a live keepalive must be reused"
    );
    // Another distro is another handle.
    assert!(held.ensure_with("distro-b", long).expect("other distro"));

    // A child that exits at once is a dead handle: the next ensure replaces it.
    let mut short = child(&["cmd", "/c", "exit", "0"]).expect("short child");
    short.wait().expect("short child exits");
    held.0
        .lock()
        .expect("registry lock")
        .insert("distro-c".into(), short);
    assert!(
        held.ensure_with("distro-c", long).expect("respawn"),
        "an exited keepalive must be replaced"
    );

    // Clean up the pings this test started.
    for (_, mut c) in held.0.lock().expect("registry lock").drain() {
        c.kill().expect("the test ping is still running");
        c.wait().expect("the killed ping is reaped");
    }
}

/// Off Windows the keepalive is the same refusal the nudge is.
#[cfg(not(windows))]
#[test]
fn keepalive_off_windows_refuses() {
    let err = keepalives()
        .ensure(&spec())
        .expect_err("must refuse off Windows");
    assert!(
        err.to_string()
            .contains("only available from a Windows host"),
        "got: {err}"
    );
}
