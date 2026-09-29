use super::*;

fn describe() -> HostOp {
    HostOp::Describe { with_token: true }
}

#[test]
fn render_describe_linux() {
    assert_eq!(
        render(Some(HostOs::Linux), &describe()).unwrap(),
        r#"sh -lc 'if [ -x "$HOME/.ralphy/bin/ralphy" ]; then "$HOME/.ralphy/bin/ralphy" daemon describe --with-token; else ralphy daemon describe --with-token; fi'"#
    );
}

#[test]
fn render_describe_macos() {
    assert_eq!(
        render(Some(HostOs::MacOs), &describe()).unwrap(),
        r#"zsh -lc 'if [ -x "$HOME/.ralphy/bin/ralphy" ]; then "$HOME/.ralphy/bin/ralphy" daemon describe --with-token; else ralphy daemon describe --with-token; fi'"#
    );
}

#[test]
fn render_describe_windows() {
    assert_eq!(
        render(Some(HostOs::Windows), &describe()).unwrap(),
        r#"if exist "%USERPROFILE%\.ralphy\bin\ralphy.exe" ("%USERPROFILE%\.ralphy\bin\ralphy.exe" daemon describe --with-token) else (ralphy daemon describe --with-token)"#
    );
}

#[test]
fn render_read_keys_windows_admin() {
    assert_eq!(
        render(Some(HostOs::Windows), &HostOp::ReadKeys { admin: true }).unwrap(),
        r"if exist C:\ProgramData\ssh\administrators_authorized_keys type C:\ProgramData\ssh\administrators_authorized_keys"
    );
}

#[test]
fn render_read_keys_windows_user() {
    assert_eq!(
        render(Some(HostOs::Windows), &HostOp::ReadKeys { admin: false }).unwrap(),
        r"if exist .ssh\authorized_keys type .ssh\authorized_keys"
    );
}

#[test]
fn render_write_keys_linux() {
    assert_eq!(
        render(Some(HostOs::Linux), &HostOp::WriteKeys { admin: false }).unwrap(),
        "sh -c 'cat > .ssh/authorized_keys'"
    );
}

#[test]
fn render_keys_skip_the_login_shell_on_macos() {
    assert_eq!(
        render(Some(HostOs::MacOs), &HostOp::ReadKeys { admin: false }).unwrap(),
        r"sh -c 'if [ -e .ssh/authorized_keys ]; then cat .ssh/authorized_keys; fi'"
    );
}

#[test]
fn render_clear_keys() {
    assert_eq!(
        render(Some(HostOs::Windows), &HostOp::ClearKeys { admin: true }).unwrap(),
        r"type nul > C:\ProgramData\ssh\administrators_authorized_keys"
    );
    assert_eq!(
        render(Some(HostOs::Linux), &HostOp::ClearKeys { admin: false }).unwrap(),
        "sh -c ': > .ssh/authorized_keys'"
    );
}

#[test]
fn render_write_keys_windows_admin() {
    assert_eq!(
        render(Some(HostOs::Windows), &HostOp::WriteKeys { admin: true }).unwrap(),
        r#"findstr "^" > C:\ProgramData\ssh\administrators_authorized_keys"#
    );
}

#[test]
fn render_set_name_quotes_inside_the_login_shell() {
    let op = HostOp::SetName {
        name: "my box".to_string(),
        avatar: 3,
    };
    assert_eq!(
        render(Some(HostOs::Linux), &op).unwrap(),
        r#"sh -lc 'if [ -x "$HOME/.ralphy/bin/ralphy" ]; then "$HOME/.ralphy/bin/ralphy" daemon setup --name '\''my box'\'' --avatar 3; else ralphy daemon setup --name '\''my box'\'' --avatar 3; fi'"#
    );
    assert_eq!(
        render(Some(HostOs::Windows), &op).unwrap(),
        r#"if exist "%USERPROFILE%\.ralphy\bin\ralphy.exe" ("%USERPROFILE%\.ralphy\bin\ralphy.exe" daemon setup --name "my box" --avatar 3) else (ralphy daemon setup --name "my box" --avatar 3)"#
    );
}

#[test]
fn render_os_detection_needs_no_os() {
    assert_eq!(render(None, &HostOp::Uname).unwrap(), "uname -s");
    assert_eq!(render(None, &HostOp::WindowsVer).unwrap(), "cmd /c ver");
    assert!(render(None, &HostOp::Probe).is_err());
}

#[test]
fn render_linger_is_linux_only() {
    assert_eq!(
        render(Some(HostOs::Linux), &HostOp::EnableLinger).unwrap(),
        "sh -lc 'loginctl enable-linger'"
    );
    assert!(render(Some(HostOs::Windows), &HostOp::EnableLinger).is_err());
}

#[test]
fn quote_posix_escapes_a_single_quote() {
    assert_eq!(quote_posix("a b'c"), r"'a b'\''c'");
    assert_eq!(quote_posix("anvil-2"), "anvil-2");
    assert_eq!(quote_posix(""), "''");
}

#[test]
fn quote_cmd_refuses_a_quote_and_a_percent() {
    assert!(quote_cmd("a\"b").is_err());
    assert!(quote_cmd("%PATH%").is_err());
    assert!(quote_cmd("a^b").is_err());
    assert!(quote_cmd("a\r\nb").is_err());
    assert_eq!(
        quote_cmd(r"C:\Program Files\x").unwrap(),
        r#""C:\Program Files\x""#
    );
    assert_eq!(quote_cmd("a&b").unwrap(), r#""a&b""#);
    assert_eq!(quote_cmd("anvil").unwrap(), "anvil");
}

#[test]
fn render_the_binary_write_skips_the_login_shell_and_prints_the_hash() {
    let linux = render(Some(HostOs::Linux), &HostOp::WriteBinary).unwrap();
    assert!(linux.starts_with("sh -c '"), "{linux}");
    assert!(
        linux.contains(r#"cat > "$HOME/.ralphy/bin/ralphy.part""#),
        "{linux}"
    );
    assert!(
        linux.contains("sha256sum") && linux.contains("shasum -a 256"),
        "{linux}"
    );

    let windows = render(Some(HostOs::Windows), &HostOp::WriteBinary).unwrap();
    assert!(windows.starts_with("powershell -NoProfile"), "{windows}");
    assert!(windows.contains("OpenStandardInput().CopyTo"), "{windows}");
    assert!(
        windows.contains("Get-FileHash -Algorithm SHA256"),
        "{windows}"
    );
    assert!(
        !windows.contains("findstr"),
        "findstr is for text: {windows}"
    );
    // cmd.exe passes the quoted script only when it holds no other `"` or `%`.
    let script = windows
        .split_once('"')
        .map(|(_, rest)| rest.trim_end_matches('"'))
        .expect("a quoted script");
    assert!(!script.contains('"') && !script.contains('%'), "{script}");
}

#[test]
fn render_the_binary_commit_sets_the_old_one_aside() {
    assert_eq!(
        render(Some(HostOs::MacOs), &HostOp::CommitBinary).unwrap(),
        r#"sh -c 'cd "$HOME/.ralphy/bin" && { if [ -e ralphy ]; then mv -f ralphy ralphy.old; fi; } && chmod +x ralphy.part && mv -f ralphy.part ralphy'"#
    );
    assert_eq!(
        render(Some(HostOs::Windows), &HostOp::CommitBinary).unwrap(),
        r#"cd /d "%USERPROFILE%\.ralphy\bin" && (if exist ralphy.exe.old del /f /q ralphy.exe.old) && (if exist ralphy.exe move /y ralphy.exe ralphy.exe.old) && move /y ralphy.exe.part ralphy.exe"#
    );
}

#[test]
fn render_the_installed_binary_never_falls_back_to_path() {
    assert_eq!(
        render(
            Some(HostOs::Linux),
            &HostOp::Installed(InstalledOp::Restart)
        )
        .unwrap(),
        r#"sh -lc '"$HOME/.ralphy/bin/ralphy" daemon restart'"#
    );
    assert_eq!(
        render(
            Some(HostOs::Windows),
            &HostOp::Installed(InstalledOp::InstallAutostart)
        )
        .unwrap(),
        r#""%USERPROFILE%\.ralphy\bin\ralphy.exe" daemon install"#
    );
}
