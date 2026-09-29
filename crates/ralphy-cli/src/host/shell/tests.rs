use super::*;

fn describe() -> HostOp {
    HostOp::Describe { with_token: true }
}

#[test]
fn render_describe_linux() {
    assert_eq!(
        render(Some(HostOs::Linux), &describe()).unwrap(),
        "sh -lc 'ralphy daemon describe --with-token'"
    );
}

#[test]
fn render_describe_macos() {
    assert_eq!(
        render(Some(HostOs::MacOs), &describe()).unwrap(),
        "zsh -lc 'ralphy daemon describe --with-token'"
    );
}

#[test]
fn render_describe_windows() {
    assert_eq!(
        render(Some(HostOs::Windows), &describe()).unwrap(),
        "ralphy daemon describe --with-token"
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
        r"sh -lc 'ralphy daemon setup --name '\''my box'\'' --avatar 3'"
    );
    assert_eq!(
        render(Some(HostOs::Windows), &op).unwrap(),
        r#"ralphy daemon setup --name "my box" --avatar 3"#
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
