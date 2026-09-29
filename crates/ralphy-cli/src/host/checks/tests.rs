use super::*;
use crate::host::ssh::tests::out;
use CheckId::*;

// format of `id -u`, `id -un` and `loginctl show-user <user> --property=Linger`
const LINUX_USER: &str = "--- uid\n1000\n--- user\npaulo\n--- linger\nLinger=yes\n";
const LINUX_ROOT: &str = "--- uid\n0\n--- user\nroot\n--- linger\nLinger=yes\n";
const LINUX_NO_LINGER: &str = "--- uid\n1000\n--- user\npaulo\n--- linger\nLinger=no\n";

// format of `fdesetup isactive`, `defaults read … autoLoginUser` (its error
// goes to stderr, so a missing key leaves the section empty) and `pmset -g`
const MACOS_FV_ON: &str = "--- uid\n501\n--- filevault\ntrue\n--- autologin\n--- pmset\n\
System-wide power settings:\nCurrently in use:\n standby              1\n \
Sleep On Power Button 1\n disksleep            10\n \
sleep                1 (sleep prevented by sharingd)\n displaysleep         10\n";
const MACOS_FV_OFF: &str = "--- uid\n501\n--- filevault\nfalse\n--- autologin\npaulo\n--- pmset\n\
Currently in use:\n disksleep            10\n sleep                0\n";

// format of `whoami /groups`, `reg query … /v AutoAdminLogon` and
// `powercfg /q SCHEME_CURRENT SUB_SLEEP STANDBYIDLE`
const WINDOWS_ADMIN: &str = "--- groups \r\n\r\nGROUP INFORMATION\r\n-----------------\r\n\r\n\
Group Name                             Type             SID          Attributes\r\n\
====================================== ================ ============ ==========\r\n\
Everyone                               Well-known group S-1-1-0      Mandatory group\r\n\
BUILTIN\\Administrators                 Alias            S-1-5-32-544 Mandatory group, Enabled group\r\n\
--- autologon \r\n\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon\r\n\
    AutoAdminLogon    REG_SZ    1\r\n\r\n--- standby \r\n\
    Current AC Power Setting Index: 0x00000000\r\n    Current DC Power Setting Index: 0x00000384\r\n";
const WINDOWS_USER: &str = "--- groups \r\n\r\nGROUP INFORMATION\r\n-----------------\r\n\
Everyone                               Well-known group S-1-1-0      Mandatory group\r\n\
BUILTIN\\Users                          Alias            S-1-5-32-545 Mandatory group\r\n\
--- autologon \r\n\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon\r\n\
    AutoAdminLogon    REG_SZ    0\r\n\r\n--- standby \r\n\
    Current AC Power Setting Index: 0x00000708\r\n";

pub(crate) fn description(os: &str) -> DaemonDescription {
    DaemonDescription {
        daemon_id: Some("01ARZ3NDEKTSV4RRFFQ69G5FC0".to_string()),
        name: Some("svrapp".to_string()),
        avatar: Some("🐙".to_string()),
        environment: "Linux".to_string(),
        os: os.to_string(),
        port: 7257,
        protocol_version: PEER_PROTOCOL_VERSION,
        require_token: true,
        autostart: true,
        running: true,
        token: None,
    }
}

fn ready(os: &str) -> RalphyOnHost {
    RalphyOnHost::Described(description(os))
}

fn run(os: HostOs, probe: &str, ralphy: &RalphyOnHost) -> Vec<HostCheck> {
    evaluate(&parse_facts(os, probe), ralphy, &[], "svrapp", None)
}

fn ids(checks: &[HostCheck]) -> Vec<CheckId> {
    checks.iter().map(|c| c.id).collect()
}

fn get(checks: &[HostCheck], id: CheckId) -> &HostCheck {
    checks
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no {id:?} in {checks:?}"))
}

#[test]
fn checks_linux_normal_user() {
    let checks = run(HostOs::Linux, LINUX_USER, &ready("linux"));
    assert_eq!(
        ids(&checks),
        [Ralphy, Name, Autostart, Linger, RequireToken, User]
    );
    for c in &checks {
        assert_eq!(c.status, CheckStatus::Pass, "{c:?}");
    }
}

#[test]
fn checks_linux_root() {
    let checks = run(HostOs::Linux, LINUX_ROOT, &ready("linux"));
    let user = get(&checks, User);
    assert_eq!(user.status, CheckStatus::Warn);
    assert!(user.text.contains("root"), "{user:?}");
}

#[test]
fn checks_linux_without_linger() {
    let checks = run(HostOs::Linux, LINUX_NO_LINGER, &ready("linux"));
    assert_eq!(
        get(&checks, Linger).status,
        CheckStatus::Fix(HostOp::EnableLinger)
    );
    assert!(!checks.iter().any(HostCheck::is_blocking));
}

#[test]
fn checks_macos_filevault_on() {
    let checks = run(HostOs::MacOs, MACOS_FV_ON, &ready("macos"));
    assert_eq!(
        ids(&checks),
        [Ralphy, Name, Autostart, SignIn, Sleep, RequireToken, User]
    );
    let sign_in = get(&checks, SignIn);
    assert_eq!(sign_in.status, CheckStatus::Warn);
    assert!(sign_in.text.contains("FileVault"), "{sign_in:?}");
    assert_eq!(
        get(&checks, Sleep).status,
        CheckStatus::Copy("sudo pmset -a sleep 0".to_string())
    );
    assert_eq!(get(&checks, User).status, CheckStatus::Pass);
}

#[test]
fn checks_macos_filevault_off() {
    let checks = run(HostOs::MacOs, MACOS_FV_OFF, &ready("macos"));
    assert_eq!(get(&checks, SignIn).status, CheckStatus::Pass);
    assert_eq!(get(&checks, Sleep).status, CheckStatus::Pass);
}

#[test]
fn checks_windows_admin() {
    let checks = run(HostOs::Windows, WINDOWS_ADMIN, &ready("windows"));
    assert_eq!(
        ids(&checks),
        [Ralphy, Name, Autostart, SignIn, Sleep, RequireToken, User]
    );
    let user = get(&checks, User);
    assert_eq!(user.status, CheckStatus::Pass);
    assert!(
        user.text.contains("administrators_authorized_keys"),
        "{user:?}"
    );
    assert_eq!(get(&checks, SignIn).status, CheckStatus::Pass);
    assert_eq!(get(&checks, Sleep).status, CheckStatus::Pass);
}

#[test]
fn checks_windows_user() {
    let checks = run(HostOs::Windows, WINDOWS_USER, &ready("windows"));
    let user = get(&checks, User);
    assert_eq!(user.status, CheckStatus::Pass);
    assert!(
        !user.text.contains("administrators_authorized_keys"),
        "{user:?}"
    );
    let sign_in = get(&checks, SignIn);
    assert_eq!(sign_in.status, CheckStatus::Warn);
    assert!(sign_in.text.contains("Autologon"), "{sign_in:?}");
    assert_eq!(
        get(&checks, Sleep).status,
        CheckStatus::Copy("powercfg /change standby-timeout-ac 0".to_string())
    );
}

#[test]
fn checks_old_ralphy() {
    let checks = run(HostOs::Linux, LINUX_USER, &RalphyOnHost::Old);
    assert_eq!(
        get(&checks, Ralphy).status,
        CheckStatus::Copy("ralphy update".to_string())
    );
    for id in [Name, Autostart, RequireToken] {
        assert_eq!(get(&checks, id).status, CheckStatus::Pending, "{id:?}");
    }
    assert!(get(&checks, Ralphy).is_blocking());
}

#[test]
fn checks_no_ralphy() {
    let checks = run(HostOs::Linux, LINUX_USER, &RalphyOnHost::Missing);
    let CheckStatus::Copy(cmd) = &get(&checks, Ralphy).status else {
        panic!("{checks:?}");
    };
    assert!(cmd.contains("releases"), "{cmd}");
    assert!(cmd.contains("Linux"), "{cmd}");
}

#[test]
fn checks_name_used_in_fleet() {
    let facts = parse_facts(HostOs::Linux, LINUX_USER);
    let fleet = vec!["anvil".to_string(), "svrapp".to_string()];
    let checks = evaluate(&facts, &ready("linux"), &fleet, "svrapp", None);
    let name = get(&checks, Name);
    assert!(matches!(name.status, CheckStatus::Copy(_)), "{name:?}");
    assert!(name.text.contains("already named svrapp"), "{name:?}");
    assert!(name.is_blocking());

    let fleet = vec!["anvil".to_string()];
    let checks = evaluate(&facts, &ready("linux"), &fleet, "svrapp", None);
    assert_eq!(get(&checks, Name).status, CheckStatus::Pass);
}

#[test]
fn checks_unnamed_host_takes_the_wanted_name() {
    let facts = parse_facts(HostOs::Linux, LINUX_USER);
    let mut d = description("linux");
    d.name = None;
    let ralphy = RalphyOnHost::Described(d);

    let checks = evaluate(&facts, &ralphy, &[], "svrapp", Some("Box-2"));
    assert_eq!(
        get(&checks, Name).status,
        CheckStatus::Fix(HostOp::SetName {
            name: "box-2".to_string(),
            avatar: 1
        })
    );

    let checks = evaluate(&facts, &ralphy, &[], "svrapp", None);
    let name = get(&checks, Name);
    assert_eq!(
        name.status,
        CheckStatus::Copy("ralphy host add svrapp --name <name>".to_string())
    );
    assert!(name.is_blocking());
}

#[test]
fn checks_describe_fixes_autostart_and_the_marker() {
    let mut d = description("linux");
    d.autostart = false;
    d.require_token = false;
    let checks = run(HostOs::Linux, LINUX_USER, &RalphyOnHost::Described(d));
    assert_eq!(
        get(&checks, Autostart).status,
        CheckStatus::Fix(HostOp::InstallAutostart)
    );
    assert_eq!(
        get(&checks, RequireToken).status,
        CheckStatus::Fix(HostOp::RequireTokenOn)
    );
}

#[test]
fn describe_with_other_protocol_is_old() {
    let mut d = description("linux");
    d.protocol_version = PEER_PROTOCOL_VERSION + 1;
    let json = serde_json::to_string(&d).unwrap();
    assert_eq!(
        classify_describe(&out(0, &json, "")).unwrap(),
        RalphyOnHost::Old
    );
    let json = serde_json::to_string(&description("linux")).unwrap();
    assert_eq!(
        classify_describe(&out(0, &json, "")).unwrap(),
        RalphyOnHost::Described(description("linux"))
    );
}

#[test]
fn describe_answers_old_and_missing() {
    // format of clap 4 and of sh / cmd.exe for an unknown command
    let old = out(
        2,
        "",
        "error: unrecognized subcommand 'describe'\n\nUsage: ralphy daemon [OPTIONS] [COMMAND]\n",
    );
    assert_eq!(classify_describe(&old).unwrap(), RalphyOnHost::Old);
    let missing = out(127, "", "sh: 1: ralphy: not found\n");
    assert_eq!(classify_describe(&missing).unwrap(), RalphyOnHost::Missing);
    let missing = out(
        1,
        "",
        "'ralphy' is not recognized as an internal or external command,\r\noperable program or batch file.\r\n",
    );
    assert_eq!(classify_describe(&missing).unwrap(), RalphyOnHost::Missing);
    assert!(classify_describe(&out(1, "", "error: reading daemon.toml")).is_err());
}

#[test]
fn describe_skips_a_login_banner_and_reads_9009_as_missing() {
    let json = serde_json::to_string(&description("linux")).unwrap();
    let banner = format!("Welcome to svrapp\n\n{json}\n");
    assert_eq!(
        classify_describe(&out(0, &banner, "")).unwrap(),
        RalphyOnHost::Described(description("linux"))
    );
    // format of a localized cmd.exe: only the exit code is stable
    let localized = out(
        9009,
        "",
        "'ralphy' não é reconhecido como um comando interno\r\n",
    );
    assert_eq!(
        classify_describe(&localized).unwrap(),
        RalphyOnHost::Missing
    );
}
