use super::*;
use crate::host::ssh::tests::out;
use CheckId::*;

// format of `id -u`, `id -un` and `loginctl show-user <user> --property=Linger`
const LINUX_USER: &str = "--- host\nsvr.example.com\n--- uid\n1000\n--- user\npaulo\n--- linger\nLinger=yes\n\
--- git\ngit version 2.43.0\n--- gh\ngh version 2.45.0 (2024-03-04)\nhttps://github.com/cli/cli/releases/tag/v2.45.0\n";
// `git --version` and `gh --version` are not found: their errors go to stderr.
const LINUX_NO_GIT: &str = "--- uid\n1000\n--- linger\nLinger=yes\n--- git\n--- gh\n";
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
        socket: None,
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
        [Ralphy, Name, Autostart, Linger, RequireToken, User, Git, Gh]
    );
    for c in &checks {
        assert_eq!(c.status, CheckStatus::Pass, "{c:?}");
    }
}

#[test]
fn a_host_without_git_gets_advice_that_does_not_block() {
    let linux = run(HostOs::Linux, LINUX_NO_GIT, &ready("linux"));
    let git = get(&linux, Git);
    assert_eq!(git.status, CheckStatus::Warn);
    assert!(git.text.contains("package manager"), "{git:?}");
    assert_eq!(get(&linux, Gh).status, CheckStatus::Warn);
    assert!(!linux.iter().any(HostCheck::is_blocking));

    let mac = run(
        HostOs::MacOs,
        "--- uid\n501\n--- git\n--- gh\n",
        &ready("macos"),
    );
    assert_eq!(
        get(&mac, Git).status,
        CheckStatus::Copy("xcode-select --install".to_string())
    );
    let windows = run(
        HostOs::Windows,
        "--- git \r\n--- gh \r\n",
        &ready("windows"),
    );
    assert_eq!(
        get(&windows, Git).status,
        CheckStatus::Copy("winget install --id Git.Git -e".to_string())
    );
}

#[test]
fn git_is_read_from_the_windows_probe() {
    let probe =
        "--- git \r\ngit version 2.47.1.windows.1\r\n--- gh \r\ngh version 2.63.0 (2024-11-27)\r\n";
    let checks = run(HostOs::Windows, probe, &ready("windows"));
    assert_eq!(get(&checks, Git).status, CheckStatus::Pass);
    assert_eq!(get(&checks, Gh).status, CheckStatus::Pass);
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
        [
            Ralphy,
            Name,
            Autostart,
            SignIn,
            Sleep,
            RequireToken,
            User,
            Git,
            Gh
        ]
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
        [
            Ralphy,
            Name,
            Autostart,
            SignIn,
            Sleep,
            RequireToken,
            User,
            Git,
            Gh
        ]
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
    let checks = run(
        HostOs::Linux,
        LINUX_USER,
        &RalphyOnHost::Old { protocol: None },
    );
    assert_eq!(
        get(&checks, Ralphy).status,
        CheckStatus::Copy("ralphy host install svrapp".to_string())
    );
    for id in [Name, Autostart, RequireToken] {
        assert_eq!(get(&checks, id).status, CheckStatus::Pending, "{id:?}");
    }
    assert!(get(&checks, Ralphy).is_blocking());
}

#[test]
fn checks_no_ralphy() {
    let checks = run(HostOs::Linux, LINUX_USER, &RalphyOnHost::Missing);
    assert_eq!(
        get(&checks, Ralphy).status,
        CheckStatus::Copy("ralphy host install svrapp".to_string())
    );
}

#[test]
fn checks_a_newer_ralphy_is_never_replaced() {
    let newer = RalphyOnHost::Old {
        protocol: Some(PEER_PROTOCOL_VERSION + 1),
    };
    assert!(newer.is_newer());
    let checks = run(HostOs::Linux, LINUX_USER, &newer);
    let ralphy = get(&checks, Ralphy);
    assert_eq!(
        ralphy.status,
        CheckStatus::Copy("ralphy update".to_string())
    );
    assert!(ralphy.text.contains("this computer"), "{ralphy:?}");
    let older = RalphyOnHost::Old {
        protocol: Some(PEER_PROTOCOL_VERSION - 1),
    };
    assert!(!older.is_newer());
    assert!(!RalphyOnHost::Old { protocol: None }.is_newer());
}

#[test]
fn the_probe_reads_the_architecture() {
    let linux = parse_facts(HostOs::Linux, "--- uid\n1000\n--- arch\nx86_64\n");
    assert_eq!(linux.arch.as_deref(), Some("x86_64"));
    let windows = parse_facts(
        HostOs::Windows,
        "--- groups \r\nx\r\n--- arch \r\nARM64\r\n",
    );
    assert_eq!(windows.arch.as_deref(), Some("ARM64"));
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
fn the_probe_reads_the_host_name_on_each_os() {
    // format of `uname -n` and `hostname`; cmd's `echo %USERNAME% &` keeps a trailing space
    assert_eq!(
        parse_facts(HostOs::Linux, LINUX_USER).host.as_deref(),
        Some("svr.example.com")
    );
    let mac = parse_facts(
        HostOs::MacOs,
        "--- host\nMacBook.local\n--- uid\n501\n--- user\npaulo\n",
    );
    assert_eq!(mac.host.as_deref(), Some("MacBook.local"));
    assert_eq!(mac.user.as_deref(), Some("paulo"));
    let win = parse_facts(
        HostOs::Windows,
        "--- host \r\nDESKTOP-1\r\n--- user \r\nPaulo \r\n--- groups \r\nx\r\n",
    );
    assert_eq!(win.host.as_deref(), Some("DESKTOP-1"));
    assert_eq!(win.user.as_deref(), Some("Paulo"));
}

fn unnamed() -> RalphyOnHost {
    let mut d = description("linux");
    d.name = None;
    RalphyOnHost::Described(d)
}

#[test]
fn an_unnamed_host_gets_its_default_name_in_the_first_run() {
    let facts = parse_facts(HostOs::Linux, LINUX_USER);
    let checks = evaluate(&facts, &unnamed(), &[], "svrapp", None);
    let name = get(&checks, Name);
    assert_eq!(
        name.status,
        CheckStatus::Fix(HostOp::SetName {
            name: "svr-paulo".to_string(),
            avatar: 1
        })
    );
    assert!(name.text.contains("svr-paulo"), "{name:?}");
    assert!(!checks.iter().any(HostCheck::is_blocking));
}

#[test]
fn the_default_name_skips_a_name_the_fleet_has() {
    let facts = parse_facts(HostOs::Linux, LINUX_USER);
    let fleet = vec!["svr-paulo".to_string()];
    let checks = evaluate(&facts, &unnamed(), &fleet, "svrapp", None);
    assert_eq!(
        get(&checks, Name).status,
        CheckStatus::Fix(HostOp::SetName {
            name: "svr-paulo-2".to_string(),
            avatar: 1
        })
    );
}

#[test]
fn wanted_name_beats_the_default() {
    let facts = parse_facts(HostOs::Linux, LINUX_USER);
    let checks = evaluate(&facts, &unnamed(), &[], "svrapp", Some("Box-2"));
    assert_eq!(
        get(&checks, Name).status,
        CheckStatus::Fix(HostOp::SetName {
            name: "box-2".to_string(),
            avatar: 1
        })
    );
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

    // no host name in the probe, so there is no default to give
    let facts = parse_facts(HostOs::Linux, "--- uid\n1000\n--- user\npaulo\n");
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
        RalphyOnHost::Old {
            protocol: Some(PEER_PROTOCOL_VERSION + 1)
        }
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
    assert_eq!(
        classify_describe(&old).unwrap(),
        RalphyOnHost::Old { protocol: None }
    );
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
