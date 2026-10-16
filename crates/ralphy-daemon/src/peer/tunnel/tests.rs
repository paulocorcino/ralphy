use super::*;
use std::process::{Command, Stdio};

fn spec(identity_file: Option<&str>) -> TunnelSpec {
    TunnelSpec {
        destination: "svrapp".into(),
        peer_port: 7257,
        peer_socket: None,
        local_port: 7401,
        identity_file: identity_file.map(str::to_string),
    }
}

#[test]
fn tunnel_argv_is_exact() {
    let ssh = Path::new("ssh");
    let base = [
        "ssh",
        "-N",
        "-L",
        "127.0.0.1:7401:127.0.0.1:7257",
        "-o",
        "BatchMode=yes",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
    ];
    let mut want: Vec<&str> = base.to_vec();
    want.extend(["--", "svrapp"]);
    assert_eq!(tunnel_argv(ssh, &spec(None)), want);

    let mut want: Vec<&str> = base.to_vec();
    want.extend(["-i", "C:/keys/ralphy", "--", "svrapp"]);
    assert_eq!(tunnel_argv(ssh, &spec(Some("C:/keys/ralphy"))), want);
}

#[test]
fn tunnel_argv_forwards_to_the_socket() {
    let mut s = spec(None);
    s.peer_port = 0;
    s.peer_socket = Some("/home/ralphy2/.ralphy/daemon.sock".into());
    let want = [
        "ssh",
        "-N",
        "-L",
        "127.0.0.1:7401:/home/ralphy2/.ralphy/daemon.sock",
        "-o",
        "BatchMode=yes",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "--",
        "svrapp",
    ];
    assert_eq!(tunnel_argv(Path::new("ssh"), &s), want);
}

/// Git for Windows puts its own `ssh.exe` on `PATH`, often first. It does not
/// read the Windows OpenSSH agent, so the system one must win.
#[cfg(windows)]
#[test]
fn windows_openssh_wins_over_a_git_ssh_first_on_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let openssh = root.join("System32").join("OpenSSH");
    let git = dir.path().join("git").join("usr").join("bin");
    std::fs::create_dir_all(&openssh).unwrap();
    std::fs::create_dir_all(&git).unwrap();
    std::fs::write(openssh.join("ssh.exe"), b"").unwrap();
    std::fs::write(git.join("ssh.exe"), b"").unwrap();
    let chosen = choose_ssh(
        Some(&root),
        Some(git.clone().into_os_string()),
        Some(".EXE".into()),
    );
    assert_eq!(chosen, Some(openssh.join("ssh.exe")));
}

#[test]
fn ssh_falls_back_to_path_without_a_system_openssh() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let name = if cfg!(windows) { "ssh.exe" } else { "ssh" };
    let ssh = bin.join(name);
    std::fs::write(&ssh, b"").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = Some(bin.clone().into_os_string());
    let ext = Some(OsString::from(".exe"));
    assert_eq!(
        choose_ssh(None, path.clone(), ext.clone()),
        Some(ssh.clone())
    );
    let empty_root = dir.path().join("root");
    assert_eq!(choose_ssh(Some(&empty_root), path, ext), Some(ssh));
}

/// Stands in for an `ssh` that holds a tunnel: it runs until it is killed.
/// Only ever started by [`an_edited_host_gets_a_new_tunnel`].
#[test]
#[ignore = "a child process of another test"]
fn tunnel_stand_in() {
    std::thread::sleep(std::time::Duration::from_secs(120));
}

fn stand_in() -> Result<Child> {
    let exe = std::env::current_exe().context("locating the test binary")?;
    Command::new(exe)
        .args([
            "--ignored",
            "--exact",
            "peer::tunnel::tests::tunnel_stand_in",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("starting the tunnel stand-in")
}

#[test]
fn an_edited_host_gets_a_new_tunnel() {
    let tunnels = Tunnels::new();
    let before = spec(None);
    let now = Instant::now();
    assert_eq!(
        tunnels
            .ensure_with("mac", &before, now, false, stand_in)
            .unwrap(),
        Ensured::Started
    );
    assert_eq!(
        tunnels
            .ensure_with("mac", &before, now, false, stand_in)
            .unwrap(),
        Ensured::Running,
        "a running tunnel with the same spec is kept"
    );
    let edited = TunnelSpec {
        destination: "user@192.168.101.3".into(),
        ..before.clone()
    };
    let old_pid = tunnels.0.lock().unwrap().held["mac"].child.id();
    assert_eq!(
        tunnels
            .ensure_with("mac", &edited, now, false, stand_in)
            .unwrap(),
        Ensured::Started
    );
    let to_socket = TunnelSpec {
        peer_port: 0,
        peer_socket: Some("/home/ralphy2/.ralphy/daemon.sock".into()),
        ..edited.clone()
    };
    assert_eq!(
        tunnels
            .ensure_with("mac", &to_socket, now, false, stand_in)
            .unwrap(),
        Ensured::Started,
        "a spec that changes from a port to a socket replaces the tunnel"
    );
    let edited = to_socket;
    let mut state = tunnels.0.lock().unwrap();
    let held = state.held.get_mut("mac").unwrap();
    assert_ne!(held.child.id(), old_pid);
    assert_eq!(held.spec, edited);
    held.child.kill().unwrap();
    held.child.wait().unwrap();
}

/// Stands in for an `ssh` that cannot reach its host: the test binary, asked
/// for a test that does not exist, exits at once.
fn exits_at_once() -> Result<Child> {
    let exe = std::env::current_exe().context("locating the test binary")?;
    Command::new(exe)
        .args(["--exact", "peer::tunnel::tests::no_such_test"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("starting the exiting stand-in")
}

/// Wait until the held `ssh` of `id` has exited.
fn wait_exited(tunnels: &Tunnels, id: &str) {
    let until = Instant::now() + Duration::from_secs(30);
    while tunnels.is_alive(id) {
        assert!(Instant::now() < until, "the stand-in did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Start an `ssh` for "mac" at `now` that exits at once, and wait for the exit.
/// The wait after an exit counts from the ensure that sees it.
fn start_and_exit(tunnels: &Tunnels, spec: &TunnelSpec, now: Instant) {
    assert_eq!(
        tunnels
            .ensure_with("mac", spec, now, false, exits_at_once)
            .unwrap(),
        Ensured::Started
    );
    wait_exited(tunnels, "mac");
}

fn waiting(secs: u64) -> Ensured {
    Ensured::Waiting {
        retry_in: Duration::from_secs(secs),
    }
}

#[test]
fn an_unreachable_peer_waits_longer_after_each_exit_up_to_ten_minutes() {
    let tunnels = Tunnels::new();
    let spec = spec(None);
    let mut now = Instant::now();
    start_and_exit(&tunnels, &spec, now);
    // The first exit is replaced at once: a short VPN drop.
    start_and_exit(&tunnels, &spec, now);
    for secs in [30, 60, 120, 240, 480, 600, 600] {
        assert_eq!(
            tunnels
                .ensure_with("mac", &spec, now, false, || panic!("it waits"))
                .unwrap(),
            waiting(secs)
        );
        now += Duration::from_secs(secs);
        start_and_exit(&tunnels, &spec, now);
    }
}

#[test]
fn an_edited_host_does_not_wait() {
    let tunnels = Tunnels::new();
    let before = spec(None);
    let now = Instant::now();
    start_and_exit(&tunnels, &before, now);
    start_and_exit(&tunnels, &before, now);
    assert_eq!(
        tunnels
            .ensure_with("mac", &before, now, false, || panic!("it waits"))
            .unwrap(),
        waiting(30)
    );
    let edited = TunnelSpec {
        destination: "user@192.168.101.3".into(),
        ..before
    };
    start_and_exit(&tunnels, &edited, now);
    // The edited host starts its own count: its first exit is replaced at once.
    start_and_exit(&tunnels, &edited, now);
}

#[test]
fn a_peer_that_answered_does_not_wait() {
    let tunnels = Tunnels::new();
    let spec = spec(None);
    let now = Instant::now();
    let waits = || {
        tunnels
            .ensure_with("mac", &spec, now, false, || panic!("it waits"))
            .unwrap()
    };
    start_and_exit(&tunnels, &spec, now);
    start_and_exit(&tunnels, &spec, now);
    assert_eq!(waits(), waiting(30));
    tunnels.answered("mac");
    // The count starts again: no wait now, and none after the next exit.
    start_and_exit(&tunnels, &spec, now);
    start_and_exit(&tunnels, &spec, now);
    assert_eq!(waits(), waiting(30));
}

/// The wake that sees the exit counts it, and still starts the `ssh` now.
#[test]
fn a_wake_does_not_wait_even_for_the_exit_it_sees() {
    let tunnels = Tunnels::new();
    let spec = spec(None);
    let now = Instant::now();
    start_and_exit(&tunnels, &spec, now);
    start_and_exit(&tunnels, &spec, now);
    assert_eq!(
        tunnels
            .ensure_with("mac", &spec, now, true, exits_at_once)
            .unwrap(),
        Ensured::Started
    );
    wait_exited(&tunnels, "mac");
    assert_eq!(
        tunnels
            .ensure_with("mac", &spec, now, false, || panic!("it waits"))
            .unwrap(),
        waiting(60),
        "the exits before the wake still count"
    );
}

/// The words of [`says_why_and_exits`].
const WHY: &str = "ssh: connect to host svrapp port 22: Connection timed out";

/// Stands in for an `ssh` that cannot reach its host and says why on stderr.
/// Only ever started by [`says_why_and_exits`].
#[test]
#[ignore = "a child process of another test"]
fn tunnel_says_why_stand_in() {
    eprintln!("{WHY}");
}

fn says_why_and_exits() -> Result<Child> {
    let exe = std::env::current_exe().context("locating the test binary")?;
    Command::new(exe)
        .args([
            "--ignored",
            "--exact",
            "peer::tunnel::tests::tunnel_says_why_stand_in",
            "--nocapture",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting the stand-in that says why")
}

#[test]
fn a_tunnel_keeps_the_last_words_of_the_ssh_before_it() {
    let tunnels = Tunnels::new();
    let before = spec(None);
    let now = Instant::now();
    // A wake never waits, so every ensure here starts an `ssh`.
    let ensure = |spec: &TunnelSpec, spawn: fn() -> Result<Child>| {
        tunnels.ensure_with("mac", spec, now, true, spawn).unwrap()
    };
    assert_eq!(ensure(&before, says_why_and_exits), Ensured::Started);
    wait_exited(&tunnels, "mac");
    assert_eq!(ensure(&before, exits_at_once), Ensured::Started);
    assert_eq!(
        tunnels.last_said("mac").as_deref(),
        Some(WHY),
        "the last line of the ssh that exited is kept"
    );
    wait_exited(&tunnels, "mac");
    assert_eq!(ensure(&before, says_why_and_exits), Ensured::Started);
    assert_eq!(
        tunnels.last_said("mac"),
        None,
        "an ssh that said nothing clears the line of the one before"
    );
    wait_exited(&tunnels, "mac");
    assert_eq!(ensure(&before, stand_in), Ensured::Started);
    assert_eq!(tunnels.last_said("mac").as_deref(), Some(WHY));
    let edited = TunnelSpec {
        destination: "user@192.168.101.3".into(),
        ..before
    };
    assert_eq!(ensure(&edited, exits_at_once), Ensured::Started);
    assert_eq!(
        tunnels.last_said("mac"),
        None,
        "what the old ssh said was about the old host"
    );
}

#[test]
fn a_close_is_quiet_only_at_the_longest_wait_with_the_same_words() {
    let said = Some("ssh: connect to host 10.1.1.4 port 22: Connection timed out".to_string());
    let other = Some("Permission denied (publickey).".to_string());
    let longest = Duration::from_secs(600);
    assert!(quiet_close(longest, said.as_ref(), said.as_ref()));
    assert!(!quiet_close(longest, said.as_ref(), other.as_ref()));
    assert!(!quiet_close(
        Duration::from_secs(480),
        said.as_ref(),
        said.as_ref()
    ));
    assert!(
        quiet_close(longest, None, None),
        "silence twice says nothing new"
    );
}

#[test]
fn a_waiting_tunnel_is_closed_and_says_when_it_tries_again() {
    use super::super::client::classify_tunnel;
    let closed = classify_tunnel("svrapp", Ok(waiting(239)), "refused".into(), None);
    assert_eq!(
        closed.state(),
        "tunnel-closed",
        "no ssh runs while it waits"
    );
    let d = closed.diagnosis("Linux");
    assert!(d.ends_with("Ralphy tries again in 4 minutes."), "got: {d}");
    let said = Some("Connection timed out".to_string());
    let soon = classify_tunnel("svrapp", Ok(waiting(30)), "refused".into(), said);
    assert_eq!(
        soon.diagnosis("Linux"),
        "The tunnel to svrapp is closed: ssh said \"Connection timed out\". Ralphy tries again in 30 seconds."
    );
    let wait = |retry_in| {
        let status = classify_tunnel("h", Ok(Ensured::Waiting { retry_in }), String::new(), None);
        status.diagnosis("Linux")
    };
    assert!(wait(Duration::from_millis(400)).ends_with("in 1 second."));
    assert!(wait(Duration::from_secs(60)).ends_with("in 1 minute."));
    assert!(wait(Duration::from_secs(600)).ends_with("in 10 minutes."));
}
