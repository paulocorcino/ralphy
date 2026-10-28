use super::*;
use crate::host::shell::{render, HostOp};
use crate::host::ssh::tests::out;

fn line(tool: Tool, name: &str, ok: bool, fix: Option<&str>) -> ToolLine {
    ToolLine {
        tool,
        name: name.to_string(),
        ok,
        required: true,
        version: None,
        text: format!("{name} text"),
        fix: fix.map(str::to_string),
    }
}

fn report(git_ok: bool) -> DoctorReport {
    DoctorReport {
        tools: vec![
            line(Tool::Git, "git", git_ok, None),
            line(Tool::Gh, "gh", false, Some("gh auth login")),
            line(Tool::Python, "python", true, None),
            line(Tool::Agent, "claude", true, None),
        ],
    }
}

fn probe_checks() -> Vec<HostCheck> {
    let check = |id| HostCheck {
        id,
        status: CheckStatus::Warn,
        text: "probe".to_string(),
    };
    vec![
        check(CheckId::User),
        check(CheckId::Git),
        check(CheckId::Gh),
    ]
}

fn ids(checks: &[HostCheck]) -> Vec<CheckId> {
    checks.iter().map(|c| c.id).collect()
}

#[test]
fn the_doctor_lines_replace_the_probe_lines_once_and_in_place() {
    let mut checks = probe_checks();
    with_doctor(&mut checks, Some(&report(true)), HostOs::Linux);
    assert_eq!(
        ids(&checks),
        [
            CheckId::User,
            CheckId::Git,
            CheckId::Gh,
            CheckId::Python,
            CheckId::Agent
        ]
    );
    assert!(checks[1..].iter().all(|c| c.text != "probe"), "{checks:?}");
}

#[test]
fn a_missing_git_keeps_the_hosts_own_install_command() {
    let git = |os| {
        let mut checks = probe_checks();
        with_doctor(&mut checks, Some(&report(false)), os);
        checks
            .into_iter()
            .find(|c| c.id == CheckId::Git)
            .expect("a git check")
            .status
    };
    assert_eq!(
        git(HostOs::MacOs),
        CheckStatus::Copy("xcode-select --install".to_string())
    );
    assert_eq!(
        git(HostOs::Windows),
        CheckStatus::Copy("winget install --id Git.Git -e".to_string())
    );
    assert_eq!(git(HostOs::Linux), CheckStatus::Warn);
}

#[test]
fn the_report_is_read_after_a_banner_and_an_unknown_tool_is_skipped() {
    let json = serde_json::to_string(&report(true)).expect("json");
    let banner = out(1, &format!("Welcome to the host\n\n{json}\n"), "");
    assert_eq!(parse_doctor(&banner), Some(report(true)));

    let newer = json.replacen(
        "{\"tool\":\"python\"",
        "{\"tool\":\"node\",\"name\":\"node\",\"ok\":true,\"required\":false,\"version\":null,\"text\":\"x\",\"fix\":null},{\"tool\":\"python\"",
        1,
    );
    let parsed = parse_doctor(&out(0, &newer, "")).expect("a report");
    assert_eq!(
        parsed,
        report(true),
        "the unknown line is skipped, the rest kept"
    );

    assert_eq!(parse_doctor(&out(0, "not json\n", "")), None);
    assert_eq!(parse_doctor(&out(2, "", "unrecognized subcommand")), None);
}

#[test]
fn doctor_is_asked_of_the_installed_ralphy_on_every_os() {
    for os in [HostOs::Linux, HostOs::MacOs, HostOs::Windows] {
        let command = render(Some(os), &HostOp::Doctor).expect("rendered");
        assert!(command.contains("doctor --json"), "{os:?}: {command}");
        assert!(command.contains(".ralphy"), "{os:?}: {command}");
    }
}

fn hooks_line(name: &str, ok: bool, text: &str) -> ToolLine {
    ToolLine {
        required: false,
        text: text.to_string(),
        ..line(Tool::Hooks, name, ok, None)
    }
}

fn hooks_of(report: &DoctorReport) -> Option<HostCheck> {
    let mut checks = probe_checks();
    with_doctor(&mut checks, Some(report), HostOs::Linux);
    checks.into_iter().find(|c| c.id == CheckId::Hooks)
}

#[test]
fn a_remote_hook_line_is_shown() {
    let mut ok = report(true);
    ok.tools.push(hooks_line("claude hooks", true, "ran"));
    let shown = hooks_of(&ok).expect("a hook check");
    assert_eq!(shown.status, CheckStatus::Pass);
    assert_eq!(shown.text, "claude hooks: ran");

    let mut failed = report(true);
    failed
        .tools
        .push(hooks_line("claude hooks", false, "ralphy did not start"));
    let shown = hooks_of(&failed).expect("a hook check");
    assert_eq!(shown.status, CheckStatus::Warn);
    assert_eq!(shown.text, "claude hooks: ralphy did not start");
    assert!(!shown.is_blocking());
}

#[test]
fn an_older_doctor_without_hook_lines_still_parses() {
    let json = serde_json::to_string(&report(true)).expect("json");
    assert!(!json.contains("\"hooks\""), "{json}");
    let parsed = parse_doctor(&out(0, &json, "")).expect("a report");
    assert_eq!(parsed, report(true));
    assert_eq!(hooks_of(&parsed), None);

    let mut newer = report(true);
    newer.tools.push(hooks_line("claude hooks", true, "ran"));
    let json = serde_json::to_string(&newer).expect("json");
    assert!(json.contains("\"tool\":\"hooks\""), "{json}");
    assert_eq!(parse_doctor(&out(0, &json, "")), Some(newer));
}
