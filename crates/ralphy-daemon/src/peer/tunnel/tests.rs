use super::*;

fn spec(identity_file: Option<&str>) -> TunnelSpec {
    TunnelSpec {
        destination: "svrapp".into(),
        peer_port: 7257,
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
